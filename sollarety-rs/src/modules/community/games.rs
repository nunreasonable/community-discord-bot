//! Jogo da pedra-papel-tesoura contra o bot e o jogo da velha entre duas
//! pessoas (Games.cs, Services/Fun/TicTacToe.cs).
//!
//! O estado do jogo da velha mora em memoria: um jogo dura minutos, e um
//! reinicio do bot no meio so faz os botoes responderem "este jogo acabou".
//! Nao vale um arquivo.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use poise::{CreateReply, serenity_prelude as serenity};
use rand::Rng;
use serenity::Builder;

use crate::{Context, Data, Error, embeds, scope, store};

const PREFIX: &str = "ttt:";
const FORFEIT_CELL: &str = "forfeit";
const IDLE_LIMIT: Duration = Duration::from_secs(180);

const LINES: [[usize; 3]; 8] = [
    [0, 1, 2],
    [3, 4, 5],
    [6, 7, 8],
    [0, 3, 6],
    [1, 4, 7],
    [2, 5, 8],
    [0, 4, 8],
    [2, 4, 6],
];

struct Game {
    x: u64,
    o: u64,
    board: [char; 9],
    x_turn: bool,
    result: Option<String>,
    winning_line: Option<[usize; 3]>,
    /// Token da interacao mais recente (comando ou clique), para editar o
    /// tabuleiro quando o prazo de inatividade vence. Vale 15 minutos.
    last_token: Option<String>,
    http: Option<Arc<serenity::Http>>,
    /// Sobe a cada jogada; o temporizador de inatividade so age se a versao
    /// capturada na hora que armou ainda for a atual - dispensa cancelar o
    /// temporizador anterior na mao.
    move_version: u64,
}

impl Game {
    fn new(x: u64, o: u64) -> Self {
        Self {
            x,
            o,
            board: [' '; 9],
            x_turn: true,
            result: None,
            winning_line: None,
            last_token: None,
            http: None,
            move_version: 0,
        }
    }

    fn current_player(&self) -> u64 {
        if self.x_turn { self.x } else { self.o }
    }
}

static GAMES: LazyLock<Mutex<HashMap<String, Arc<Mutex<Game>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn create(x: u64, o: u64) -> (String, Arc<Mutex<Game>>) {
    let mut games = GAMES.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        // 8 hex bastam para distinguir os jogos em andamento, e cabem
        // folgado nos 100 caracteres de um custom id.
        let id = store::short_id(8);
        if !games.contains_key(&id) {
            let game = Arc::new(Mutex::new(Game::new(x, o)));
            games.insert(id.clone(), game.clone());
            return (id, game);
        }
    }
}

fn find(id: &str) -> Option<Arc<Mutex<Game>>> {
    GAMES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
        .cloned()
}

fn end(id: &str) {
    GAMES.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
}

/// Joga na casa; devolve false se a casa ja estava ocupada.
fn play(game: &mut Game, cell: usize) -> bool {
    if game.board[cell] != ' ' {
        return false;
    }

    let mark = if game.x_turn { 'X' } else { 'O' };
    game.board[cell] = mark;

    if let Some(&line) = LINES
        .iter()
        .find(|line| line.iter().all(|&i| game.board[i] == mark))
    {
        game.winning_line = Some(line);
        game.result = Some(format!("🎉 <@{}> wins!", game.current_player()));
        return true;
    }

    if game.board.iter().all(|&c| c != ' ') {
        game.result = Some("🤝 It's a draw!".to_owned());
        return true;
    }

    game.x_turn = !game.x_turn;
    true
}

fn content(game: &Game) -> String {
    let header = format!("❌ <@{}> vs ⭕ <@{}>", game.x, game.o);
    let status = game.result.clone().unwrap_or_else(|| {
        format!(
            "It's <@{}>'s turn ({}).",
            game.current_player(),
            if game.x_turn { "❌" } else { "⭕" }
        )
    });
    format!("{header}\n\n{status}")
}

/// O tabuleiro em tres fileiras de botoes, mais a de desistir.
fn board_components(game: &Game, id: &str) -> Vec<serenity::CreateActionRow> {
    let over = game.result.is_some();
    let mut rows = Vec::with_capacity(4);

    for row in 0..3 {
        let mut buttons = Vec::with_capacity(3);
        for col in 0..3 {
            let cell = row * 3 + col;
            let mark = game.board[cell];
            let winning = game.winning_line.is_some_and(|line| line.contains(&cell));

            let style = if winning {
                serenity::ButtonStyle::Success
            } else {
                match mark {
                    'X' => serenity::ButtonStyle::Primary,
                    _ => serenity::ButtonStyle::Secondary,
                }
            };

            let mut button = serenity::CreateButton::new(format!("{PREFIX}{id}:{cell}"))
                .style(style)
                .disabled(over || mark != ' ');
            button = match mark {
                // "\u{200b}" e o rotulo vazio: botao exige rotulo ou emoji, e
                // a casa livre nao tem nenhum dos dois.
                ' ' => button.label("\u{200b}"),
                'X' => button.emoji(serenity::ReactionType::Unicode("✖️".into())),
                _ => button.emoji(serenity::ReactionType::Unicode("⭕".into())),
            };
            buttons.push(button);
        }
        rows.push(serenity::CreateActionRow::Buttons(buttons));
    }

    rows.push(serenity::CreateActionRow::Buttons(vec![
        serenity::CreateButton::new(format!("{PREFIX}{id}:{FORFEIT_CELL}"))
            .style(serenity::ButtonStyle::Secondary)
            .label("Give up")
            .emoji(serenity::ReactionType::Unicode("🏳️".into()))
            .disabled(over),
    ]));

    rows
}

/// Rearma o prazo de inatividade. Estourou, o jogo termina e o tabuleiro e
/// desligado pela ultima interacao conhecida.
fn arm_idle_timer(id: String, game: Arc<Mutex<Game>>) {
    let version = {
        let mut g = game.lock().unwrap_or_else(|e| e.into_inner());
        g.move_version += 1;
        g.move_version
    };

    tokio::spawn(async move {
        tokio::time::sleep(IDLE_LIMIT).await;

        let (http, token) = {
            let mut g = game.lock().unwrap_or_else(|e| e.into_inner());
            if g.move_version != version || g.result.is_some() {
                return;
            }
            g.result = Some(format!(
                "⌛ Game over — <@{}> didn't move for {} minutes.",
                g.current_player(),
                IDLE_LIMIT.as_secs() / 60
            ));
            (g.http.clone(), g.last_token.clone())
        };

        end(&id);

        let (Some(http), Some(token)) = (http, token) else {
            return;
        };

        let (board_content, board_rows) = {
            let g = game.lock().unwrap_or_else(|e| e.into_inner());
            (content(&g), board_components(&g, &id))
        };

        let update = serenity::EditInteractionResponse::new()
            .content(board_content)
            .components(board_rows);
        if let Err(cause) = update.execute(http, &token).await {
            tracing::warn!(
                "[tictactoe] aviso: nao consegui encerrar o jogo {id} expirado: {cause}"
            );
        }
    });
}

// ------------------------------------------------------------------
// Comandos
// ------------------------------------------------------------------

const ROCK: &str = "rock";
const PAPER: &str = "paper";
const SCISSORS: &str = "scissors";
const MOVES: &[&str] = &[ROCK, PAPER, SCISSORS];

fn show(mv: &str) -> &'static str {
    match mv {
        ROCK => "🪨 **Rock**",
        PAPER => "📄 **Paper**",
        _ => "✂️ **Scissors**",
    }
}

#[derive(poise::ChoiceParameter)]
pub enum Move {
    #[name = "🪨 Rock"]
    Rock,
    #[name = "📄 Paper"]
    Paper,
    #[name = "✂️ Scissors"]
    Scissors,
}

impl Move {
    fn as_str(&self) -> &'static str {
        match self {
            Move::Rock => ROCK,
            Move::Paper => PAPER,
            Move::Scissors => SCISSORS,
        }
    }
}

/// Play rock, paper, scissors against me
#[poise::command(
    slash_command,
    rename = "rps",
    check = "crate::cooldown::user::<3, 10>"
)]
pub async fn rps(ctx: Context<'_>, #[description = "Your move"] mv: Move) -> Result<(), Error> {
    let you = mv.as_str();
    let mine = MOVES[rand::rng().random_range(0..MOVES.len())];

    let you_win = matches!(
        (you, mine),
        (ROCK, SCISSORS) | (PAPER, ROCK) | (SCISSORS, PAPER)
    );
    let (verdict, color) = if you == mine {
        ("🤝 **It's a draw!**", embeds::colors::GOLD)
    } else if you_win {
        ("🎉 **You win!**", embeds::colors::SPRING_GREEN)
    } else {
        // Derrota nao e erro: sem vermelho (ver embeds.rs).
        ("😎 **I win!**", embeds::colors::BLURPLE)
    };

    let embed = serenity::CreateEmbed::new()
        .title("Rock, paper, scissors!")
        .description(format!(
            "You chose {}, I chose {}.\n\n{verdict}",
            show(you),
            show(mine)
        ))
        .colour(color);
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Challenge someone to a game of tic-tac-toe
#[poise::command(
    slash_command,
    rename = "tictactoe",
    check = "crate::cooldown::user::<2, 30>"
)]
pub async fn tictactoe(
    ctx: Context<'_>,
    #[description = "Who to play against"] opponent: serenity::User,
) -> Result<(), Error> {
    let refusal = if opponent.id == ctx.author().id {
        Some("You can't play against yourself. Pick someone else!")
    } else if opponent.bot {
        Some("Bots can't click buttons. Challenge a person instead.")
    } else {
        None
    };

    if let Some(refusal) = refusal {
        ctx.send(
            CreateReply::default()
                .embed(embeds::error("Can't start the game", refusal))
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    // Quem desafiou comeca com o X.
    let (id, game_arc) = create(ctx.author().id.get(), opponent.id.get());

    let token = interaction_token(ctx);
    let http = ctx.serenity_context().http.clone();
    let (board_content, board_rows) = {
        let mut g = game_arc.lock().unwrap_or_else(|e| e.into_inner());
        g.last_token = token;
        g.http = Some(http);
        (content(&g), board_components(&g, &id))
    };

    // So o adversario e notificado: e ele quem precisa saber que o jogo
    // comecou. As edicoes seguintes nao notificam ninguem.
    ctx.send(
        CreateReply::default()
            .content(board_content)
            .components(board_rows)
            .allowed_mentions(serenity::CreateAllowedMentions::new().users([opponent.id])),
    )
    .await?;

    arm_idle_timer(id, game_arc);
    Ok(())
}

fn interaction_token(ctx: Context<'_>) -> Option<String> {
    match ctx {
        poise::Context::Application(app) => Some(app.interaction.token.clone()),
        poise::Context::Prefix(_) => None,
    }
}

// ------------------------------------------------------------------
// Botao "ttt:"
// ------------------------------------------------------------------

pub async fn on_component(
    ctx: &serenity::Context,
    press: &serenity::ComponentInteraction,
) -> Result<(), Error> {
    let id = press.data.custom_id.as_str();
    if !id.starts_with(PREFIX) {
        return Ok(());
    }

    let parts: Vec<&str> = id[PREFIX.len()..].split(':').collect();
    let Some(game_arc) = parts.first().and_then(|&gid| find(gid)) else {
        press
            .create_response(
                ctx,
                serenity::CreateInteractionResponse::Message(
                    serenity::CreateInteractionResponseMessage::new()
                        .embed(embeds::info(
                            "Game over",
                            "This game already ended. Start a new one with `/tictactoe`.",
                        ))
                        .ephemeral(true),
                ),
            )
            .await?;
        return Ok(());
    };
    let game_id = parts[0].to_owned();
    let user_id = press.user.id.get();

    let mut refusal = None;
    let finished;
    {
        let mut game = game_arc.lock().unwrap_or_else(|e| e.into_inner());
        if game.result.is_some() {
            refusal = Some("This game already ended.");
        } else if user_id != game.x && user_id != game.o {
            refusal = Some("This isn't your game. Start your own with `/tictactoe`.");
        } else if parts.get(1) == Some(&FORFEIT_CELL) {
            let winner = if user_id == game.x { game.o } else { game.x };
            game.result = Some(format!("🏳️ <@{user_id}> gave up — <@{winner}> wins!"));
        } else if user_id != game.current_player() {
            refusal = Some("Wait for your turn.");
        } else {
            match parts.get(1).and_then(|s| s.parse::<usize>().ok()) {
                Some(cell) if cell <= 8 => {
                    if !play(&mut game, cell) {
                        refusal = Some("That square is already taken.");
                    }
                }
                _ => refusal = Some("That square doesn't exist."),
            }
        }

        if refusal.is_none() {
            game.last_token = Some(press.token.clone());
            game.http = Some(ctx.http.clone());
        }
        finished = game.result.is_some();
    }

    if let Some(refusal) = refusal {
        press
            .create_response(
                ctx,
                serenity::CreateInteractionResponse::Message(
                    serenity::CreateInteractionResponseMessage::new()
                        .embed(embeds::info("Can't do that", refusal))
                        .ephemeral(true),
                ),
            )
            .await?;
        return Ok(());
    }

    if finished {
        end(&game_id);
    } else {
        arm_idle_timer(game_id.clone(), game_arc.clone());
    }

    let (board_content, board_rows) = {
        let game = game_arc.lock().unwrap_or_else(|e| e.into_inner());
        (content(&game), board_components(&game, &game_id))
    };

    press
        .create_response(
            ctx,
            serenity::CreateInteractionResponse::UpdateMessage(
                serenity::CreateInteractionResponseMessage::new()
                    .content(board_content)
                    .components(board_rows)
                    .allowed_mentions(serenity::CreateAllowedMentions::new()),
            ),
        )
        .await?;
    Ok(())
}

pub fn commands() -> Vec<poise::Command<Data, Error>> {
    vec![scope::anywhere(rps()), scope::anywhere(tictactoe())]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joga_e_detecta_vitoria_na_linha() {
        let mut game = Game::new(1, 2);
        assert!(play(&mut game, 0)); // X
        assert!(play(&mut game, 3)); // O
        assert!(play(&mut game, 1)); // X
        assert!(play(&mut game, 4)); // O
        assert!(play(&mut game, 2)); // X vence a linha 0,1,2
        assert_eq!(game.result, Some("🎉 <@1> wins!".to_owned()));
        assert_eq!(game.winning_line, Some([0, 1, 2]));
    }

    #[test]
    fn nao_deixa_jogar_em_casa_ocupada() {
        let mut game = Game::new(1, 2);
        assert!(play(&mut game, 0));
        assert!(!play(&mut game, 0));
    }

    #[test]
    fn empate_quando_tabuleiro_enche_sem_vencedor() {
        let mut game = Game::new(1, 2);
        // X O X / X O O / O X X -> sem linha completa, tabuleiro cheio.
        for cell in [0, 1, 2, 4, 3, 5, 7, 6, 8] {
            play(&mut game, cell);
        }
        assert_eq!(game.result, Some("🤝 It's a draw!".to_owned()));
    }

    #[test]
    fn alterna_o_turno() {
        let mut game = Game::new(10, 20);
        assert_eq!(game.current_player(), 10);
        play(&mut game, 0);
        assert_eq!(game.current_player(), 20);
    }
}
