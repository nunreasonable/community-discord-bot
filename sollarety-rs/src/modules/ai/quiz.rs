//! /quiz: tema vira perguntas via IA local (um pedido so, com json_schema),
//! cada pergunta e um round de 20s por botao, e quem acerta primeiro leva
//! SOL$ (Services/Ai/AiQuiz.cs). Nao tem a ver com o personagem Sollarety -
//! usa `local::ask`, nao o `complete` do chat por mencao.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use poise::serenity_prelude as serenity;
use tokio::sync::oneshot;

use super::{local, quiz_schema::QuizQuestion};
use crate::{Data, config::LocalAiSettings, embeds, store};

pub const PREFIX: &str = "quiz:";
pub const REWARD: i64 = 50;

const QUESTION_TIME: Duration = Duration::from_secs(20);
const CHANNEL_COOLDOWN: chrono::Duration = chrono::Duration::minutes(10);

/// Um quiz em andamento num canal. Mora em memoria: um reinicio do bot no
/// meio so faz os botoes pararem de responder, igual ao jogo da velha.
pub struct Game {
    pub id: String,
    pub channel_id: u64,
    inner: Mutex<GameInner>,
}

struct GameInner {
    questions: Vec<QuizQuestion>,
    paid: bool,
    /// None antes da primeira pergunta (equivalente ao -1 do C#).
    current_index: Option<usize>,
    winner: Option<u64>,
    /// Quem ja errou a pergunta atual - fora da rodada, mas pode tentar a proxima.
    wrong_users: HashSet<u64>,
    /// Resolvido na hora que alguem acerta, para o laco da pergunta nao esperar os 20s inteiros.
    resolved_tx: Option<oneshot::Sender<()>>,
    /// Quantas rodadas cada pessoa ganhou, na ordem de quem ganhou primeiro.
    wins: IndexMap<u64, u32>,
}

impl Game {
    fn new(id: String, channel_id: u64) -> Self {
        Self {
            id,
            channel_id,
            inner: Mutex::new(GameInner {
                questions: Vec::new(),
                paid: false,
                current_index: None,
                winner: None,
                wrong_users: HashSet::new(),
                resolved_tx: None,
                wins: IndexMap::new(),
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GameInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_questions(&self, questions: Vec<QuizQuestion>, paid: bool) {
        let mut inner = self.lock();
        inner.questions = questions;
        inner.paid = paid;
    }

    pub fn question_count(&self) -> usize {
        self.lock().questions.len()
    }
}

pub enum QuizReserve {
    Ok(Arc<Game>),
    Active,
    Cooldown(DateTime<Utc>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuizClick {
    Won,
    Wrong,
    LockedOut,
    Stale,
    AlreadyDecided,
}

// Um jogo por canal (chave de reserva) e tambem por id (chave do botao, que
// nao carrega o canal). Os tres mapas vivem atras de UM mutex so, para a
// reserva (cooldown + canal + id) ser atomica, como o C# consegue com tres
// ConcurrentDictionary porque cada checagem la e isolada por TryAdd.
struct Registry {
    by_channel: HashMap<u64, Arc<Game>>,
    by_id: HashMap<String, Arc<Game>>,
    cooldown_until: HashMap<u64, DateTime<Utc>>,
}

static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(|| {
    Mutex::new(Registry {
        by_channel: HashMap::new(),
        by_id: HashMap::new(),
        cooldown_until: HashMap::new(),
    })
});

fn registry() -> std::sync::MutexGuard<'static, Registry> {
    REGISTRY.lock().unwrap_or_else(|e| e.into_inner())
}

/// Tenta comecar um quiz neste canal.
pub fn try_reserve(channel_id: u64) -> QuizReserve {
    let mut reg = registry();

    if let Some(&until) = reg.cooldown_until.get(&channel_id)
        && until > Utc::now()
    {
        return QuizReserve::Cooldown(until);
    }

    if reg.by_channel.contains_key(&channel_id) {
        return QuizReserve::Active;
    }

    let mut id = store::short_id(8);
    while reg.by_id.contains_key(&id) {
        id = store::short_id(8);
    }

    let game = Arc::new(Game::new(id.clone(), channel_id));
    reg.by_channel.insert(channel_id, game.clone());
    reg.by_id.insert(id, game.clone());
    QuizReserve::Ok(game)
}

/// A IA nao devolveu um quiz valido: sai do mapa sem acender cooldown, pra
/// poder tentar de novo na hora.
pub fn cancel(game: &Game) {
    let mut reg = registry();
    reg.by_channel.remove(&game.channel_id);
    reg.by_id.remove(&game.id);
}

/// Quiz jogado ate o fim: sai do mapa e liga os 10 minutos de cooldown do canal.
fn finish(game: &Game) {
    let mut reg = registry();
    reg.by_channel.remove(&game.channel_id);
    reg.by_id.remove(&game.id);
    reg.cooldown_until
        .insert(game.channel_id, Utc::now() + CHANNEL_COOLDOWN);
}

pub fn find(id: &str) -> Option<Arc<Game>> {
    registry().by_id.get(id).cloned()
}

/// Um pedido so para TODAS as perguntas, via json_schema. Tenta de novo uma
/// vez se sair invalido.
pub async fn generate(
    settings: &LocalAiSettings,
    theme: &str,
    count: usize,
) -> Option<Vec<QuizQuestion>> {
    let schema = super::quiz_schema::build(count);
    let language = if local::is_english(theme) {
        "Write entirely in English."
    } else {
        "Escreva tudo em português do Brasil."
    };
    let system = format!(
        "You write short trivia quizzes. Reply ONLY with JSON matching the given schema: an object \
         with a \"questions\" array of exactly {count} item(s). Each item has \"question\" (one \
         factual question), \"options\" (exactly 4 short, distinct answers) and \"answer\" (the \
         0-based index of the single correct option). Keep every question unambiguous and strictly \
         factual - no opinions, no trick questions. Avoid offensive, sexual, violent or otherwise \
         sensitive topics; if the theme leans that way, write a safer, more general question about \
         it instead. No text outside the JSON object. {language}"
    );
    let user = format!("Theme: {theme}");
    let max_tokens = 220 * count as i32 + 200;

    // Uma tentativa e mais uma: o suficiente para um tropeco de formatacao
    // sem deixar quem pediu esperando varias rodadas de pedido.
    for _ in 0..2 {
        let raw = local::ask(settings, &system, &user, max_tokens, Some(schema.clone())).await;
        if let Some(parsed) = super::quiz_schema::parse_and_validate(raw.as_deref(), count) {
            return Some(parsed);
        }
    }
    None
}

/// Decide o resultado sob o lock do jogo - sincrono de proposito, porque o
/// lock so protege o estado em memoria. Premio e resposta ao Discord ficam
/// para depois, fora dele.
pub fn resolve(game: &Game, question_index: usize, option_index: usize, user_id: u64) -> QuizClick {
    let mut inner = game.lock();

    if Some(question_index) != inner.current_index {
        return QuizClick::Stale;
    }
    if inner.winner.is_some() {
        return QuizClick::AlreadyDecided;
    }
    if inner.wrong_users.contains(&user_id) {
        return QuizClick::LockedOut;
    }

    if inner.questions[question_index].answer != option_index {
        inner.wrong_users.insert(user_id);
        return QuizClick::Wrong;
    }

    inner.winner = Some(user_id);
    *inner.wins.entry(user_id).or_insert(0) += 1;
    if let Some(tx) = inner.resolved_tx.take() {
        let _ = tx.send(());
    }
    QuizClick::Won
}

/// Toca o quiz inteiro, uma pergunta apos a outra, e no final posta o
/// placar. Chamado em segundo plano pelo /quiz - a interacao ja foi
/// respondida antes disto comecar.
pub async fn run(
    http: Arc<serenity::Http>,
    channel_id: serenity::ChannelId,
    game: Arc<Game>,
    data: Data,
) {
    let total = game.question_count();
    let paid = game.lock().paid;

    for i in 0..total {
        let rx = {
            let mut inner = game.lock();
            inner.current_index = Some(i);
            inner.winner = None;
            inner.wrong_users.clear();
            let (tx, rx) = oneshot::channel();
            inner.resolved_tx = Some(tx);
            rx
        };

        let question = game.lock().questions[i].clone();
        let mut message = match channel_id
            .send_message(&http, question_message(&game.id, i, total, &question, paid))
            .await
        {
            Ok(message) => message,
            Err(cause) => {
                tracing::error!(
                    "[quiz] nao consegui postar a pergunta {} no canal {channel_id}: {cause}",
                    i + 1
                );
                break;
            }
        };

        tokio::select! {
            _ = rx => {}
            () = tokio::time::sleep(QUESTION_TIME) => {}
        }

        let winner_id = game.lock().winner;

        if paid
            && let Some(winner_id) = winner_id
            && let Err(cause) = data.economy.award(winner_id, REWARD).await
        {
            tracing::error!("[quiz] nao consegui creditar o premio de {winner_id}: {cause}");
        }

        if let Err(cause) = message
            .edit(
                &http,
                reveal_message(&game.id, i, total, &question, winner_id, paid),
            )
            .await
        {
            tracing::error!(
                "[quiz] nao consegui revelar a resposta da pergunta {}: {cause}",
                i + 1
            );
        }
    }

    // Sempre libera o canal e liga o cooldown, mesmo se uma pergunta falhou
    // no meio - sem isto um erro de rede deixava o canal trancado "com um
    // quiz rodando" para sempre.
    finish(&game);

    if let Err(cause) = channel_id
        .send_message(&http, scoreboard_message(&game))
        .await
    {
        tracing::error!("[quiz] nao consegui postar o placar final: {cause}");
    }
}

fn option_label(index: usize, text: &str) -> String {
    let letter = (b'A' + index as u8) as char;
    embeds::trim(&format!("{letter}) {text}"), 80)
}

fn question_message(
    game_id: &str,
    index: usize,
    total: usize,
    question: &QuizQuestion,
    paid: bool,
) -> serenity::CreateMessage {
    let footer = if paid {
        format!(
            "20 seconds - first correct answer wins {}",
            store::economy::format(REWARD)
        )
    } else {
        "20 seconds - first correct answer gets the point".to_owned()
    };

    let embed = serenity::CreateEmbed::new()
        .title(format!("❓ Question {}/{total}", index + 1))
        .description(embeds::safe_trim(&question.question, 2000))
        .colour(embeds::colors::BLURPLE)
        .footer(serenity::CreateEmbedFooter::new(footer));

    let buttons: Vec<serenity::CreateButton> = question
        .options
        .iter()
        .enumerate()
        .map(|(i, opt)| {
            serenity::CreateButton::new(format!("{PREFIX}{game_id}:{index}:{i}"))
                .style(serenity::ButtonStyle::Secondary)
                .label(option_label(i, opt))
        })
        .collect();

    serenity::CreateMessage::new()
        .embed(embed)
        .components(vec![serenity::CreateActionRow::Buttons(buttons)])
        .allowed_mentions(serenity::CreateAllowedMentions::new())
}

fn reveal_message(
    game_id: &str,
    index: usize,
    total: usize,
    question: &QuizQuestion,
    winner_id: Option<u64>,
    paid: bool,
) -> serenity::EditMessage {
    let mut lines = vec![embeds::safe_trim(&question.question, 1000), String::new()];
    for (i, opt) in question.options.iter().enumerate() {
        let mark = if i == question.answer { "✅" } else { "⬜" };
        let letter = (b'A' + i as u8) as char;
        lines.push(format!("{mark} {letter}) {}", embeds::safe_trim(opt, 150)));
    }
    lines.push(String::new());
    lines.push(match winner_id {
        Some(id) if paid => format!(
            "🏆 <@{id}> got it first — +{}!",
            store::economy::format(REWARD)
        ),
        Some(id) => format!("🏆 <@{id}> got it first!"),
        None => "⌛ Time's up, nobody got it.".to_owned(),
    });

    let embed = serenity::CreateEmbed::new()
        .title(format!("❓ Question {}/{total} — answered", index + 1))
        .description(lines.join("\n"))
        .colour(if winner_id.is_some() {
            embeds::colors::SPRING_GREEN
        } else {
            embeds::colors::BLURPLE
        });

    let buttons: Vec<serenity::CreateButton> = question
        .options
        .iter()
        .enumerate()
        .map(|(i, opt)| {
            let style = if i == question.answer {
                serenity::ButtonStyle::Success
            } else {
                serenity::ButtonStyle::Secondary
            };
            serenity::CreateButton::new(format!("{PREFIX}{game_id}:{index}:{i}"))
                .style(style)
                .label(option_label(i, opt))
                .disabled(true)
        })
        .collect();

    serenity::EditMessage::new()
        .embed(embed)
        .components(vec![serenity::CreateActionRow::Buttons(buttons)])
        .allowed_mentions(serenity::CreateAllowedMentions::new())
}

fn scoreboard_message(game: &Game) -> serenity::CreateMessage {
    let mut ranked: Vec<(u64, u32)> = {
        let inner = game.lock();
        inner
            .wins
            .iter()
            .map(|(&user, &wins)| (user, wins))
            .collect()
    };
    // Sort estavel: empate mantem a ordem de quem ganhou a primeira rodada
    // primeiro, igual ao OrderByDescending (estavel) do LINQ.
    ranked.sort_by_key(|&(_, wins)| std::cmp::Reverse(wins));

    let description = if ranked.is_empty() {
        "Nobody got a question right this time.".to_owned()
    } else {
        ranked
            .iter()
            .enumerate()
            .map(|(i, (user, wins))| format!("**{}.** <@{user}> — {wins} correct", i + 1))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let embed = serenity::CreateEmbed::new()
        .title("🏁 Quiz over — scoreboard")
        .description(description)
        .colour(embeds::colors::GOLD);

    serenity::CreateMessage::new()
        .embed(embed)
        .allowed_mentions(serenity::CreateAllowedMentions::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_questions() -> Vec<QuizQuestion> {
        vec![QuizQuestion {
            question: "2 + 2?".to_owned(),
            options: ["3".into(), "4".into(), "5".into(), "6".into()],
            answer: 1,
        }]
    }

    #[test]
    fn resolve_premia_quem_acerta_primeiro() {
        let game = Game::new("abc".into(), 1);
        game.set_questions(sample_questions(), false);
        {
            let mut inner = game.lock();
            inner.current_index = Some(0);
        }

        assert_eq!(resolve(&game, 0, 0, 10), QuizClick::Wrong);
        assert_eq!(resolve(&game, 0, 1, 10), QuizClick::LockedOut); // ja errou, trancado
        assert_eq!(resolve(&game, 0, 1, 20), QuizClick::Won);
        assert_eq!(resolve(&game, 0, 1, 30), QuizClick::AlreadyDecided);
        // Pergunta de outra rodada: clique chega atrasado.
        assert_eq!(resolve(&game, 1, 1, 40), QuizClick::Stale);
    }

    #[test]
    fn reserva_e_atomica_por_canal() {
        let channel = rand::random::<u64>();
        let first = try_reserve(channel);
        assert!(matches!(first, QuizReserve::Ok(_)));
        assert!(matches!(try_reserve(channel), QuizReserve::Active));

        if let QuizReserve::Ok(game) = first {
            cancel(&game);
        }
        // Cancelado: pode reservar de novo na hora, sem cooldown.
        assert!(matches!(try_reserve(channel), QuizReserve::Ok(_)));
    }

    #[test]
    fn finish_liga_o_cooldown_do_canal() {
        let channel = rand::random::<u64>();
        let QuizReserve::Ok(game) = try_reserve(channel) else {
            panic!("reserva deveria funcionar");
        };
        finish(&game);
        assert!(matches!(try_reserve(channel), QuizReserve::Cooldown(_)));
    }
}
