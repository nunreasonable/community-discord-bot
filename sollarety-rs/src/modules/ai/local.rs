//! Conversa por IA local, com o mesmo modelo do ccore (llama.cpp na porta
//! 9931 por padrao). Quem liga e desliga o modelo e o ccore: este bot so
//! responde enquanto o servidor estiver de pe, e cai junto quando ele
//! desliga (LocalAi.cs).
//!
//! So responde quando mencionado (reply com ping conta), e so nos servidores
//! da lista local_ai.guild_ids. Sem o intent de Message Content o Discord ja
//! so entrega o texto das mensagens que mencionam o bot, entao nao ha
//! historico do canal: o contexto e a mensagem e, se for reply a este bot, a
//! mensagem dele.

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use poise::serenity_prelude as serenity;
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use super::guard;
use crate::{Data, config::LocalAiSettings, embeds};

/// Teto do Discord e 2000; sobra margem.
const MAX_REPLY: usize = 1900;

/// Cliente proprio, SEM a trava de endereco interno do compartilhado: aqui o
/// endereco local (127.0.0.1) e exatamente o alvo. Timeout generoso: o
/// modelo pode levar para responder.
static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    crate::http::local_client(Duration::from_secs(120)).unwrap_or_else(|cause| {
        tracing::error!("[ia] falha ao montar o cliente http local: {cause}");
        reqwest::Client::new()
    })
});

/// Um pedido por vez: o servidor tem um slot so para este bot.
static BUSY: Semaphore = Semaphore::const_new(1);

static LAST_ASK: LazyLock<Mutex<HashMap<u64, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Chamado pelo on_event do modulo para TODA mensagem. So dispara o
/// tratamento (em segundo plano, como o Task.Run do C#) quando faz sentido:
/// bloquear aqui travaria a entrega de evento do serenity para o resto do
/// bot.
pub fn on_message_create(ctx: &serenity::Context, data: &Data, msg: &serenity::Message) {
    if msg.author.bot || msg.webhook_id.is_some() {
        return;
    }
    if !matches!(
        msg.kind,
        serenity::MessageType::Regular | serenity::MessageType::InlineReply
    ) {
        return;
    }
    let Some(guild_id) = msg.guild_id else {
        return;
    };
    if !mentioned(ctx, msg) {
        return;
    }

    let ctx = ctx.clone();
    let data = data.clone();
    let msg = msg.clone();
    tokio::spawn(async move {
        handle(&ctx, &data, &msg, guild_id.get()).await;
    });
}

/// Mencao ao bot (usuario ou o cargo dele, que o Discord oferece sozinho) ou
/// reply a uma mensagem dele conta como "falou com o bot" no C# via
/// MentionedUsers/MentionedRoles; aqui o reply entra no proprio `mentions`
/// porque o Discord inclui o autor da mensagem respondida quando a mencao
/// (ping) esta ligada, que e o padrao do cliente.
fn mentioned(ctx: &serenity::Context, msg: &serenity::Message) -> bool {
    let me = ctx.cache.current_user().id;
    if msg.mentions_user_id(me) {
        return true;
    }
    msg.guild_id.is_some_and(|guild| {
        ctx.cache.guild(guild).is_some_and(|g| {
            msg.mention_roles.iter().any(|id| {
                g.roles
                    .get(id)
                    .is_some_and(|role| role.tags.bot_id == Some(me))
            })
        })
    })
}

async fn handle(ctx: &serenity::Context, data: &Data, msg: &serenity::Message, guild_id: u64) {
    let settings = data.config().local_ai.clone();
    if !settings.guild_ids.contains(&guild_id) {
        return;
    }

    let me = ctx.cache.current_user().id.get();
    let text = strip_mention(&msg.content, me);
    if text.is_empty() {
        return;
    }

    // Modelo desligado: finge que nada aconteceu, como o ccore.
    if !is_up(&settings).await {
        return;
    }

    let author_id = msg.author.id.get();
    let is_operator = settings.operator_user_ids.contains(&author_id);

    if !is_operator {
        let verdict = guard::judge(author_id, &text, Instant::now());
        if verdict == guard::Verdict::Ignore {
            return;
        }
        if let Some(fixed) = guard::telling_off(verdict, is_english(&text)) {
            send_reply(ctx, msg, fixed).await;
            return;
        }

        let now = Instant::now();
        let mut last_ask = LAST_ASK.lock().unwrap_or_else(|e| e.into_inner());
        let wait = Duration::from_secs(settings.cooldown_seconds.max(0) as u64);
        if let Some(&last) = last_ask.get(&author_id)
            && now.duration_since(last) < wait
        {
            return;
        }
        last_ask.insert(author_id, now);
        drop(last_ask);
    }

    let Ok(Ok(_permit)) = tokio::time::timeout(Duration::from_secs(60), BUSY.acquire()).await
    else {
        return;
    };
    let _typing = msg.channel_id.start_typing(&ctx.http);
    let mut reply = complete(&settings, msg, &text, me).await;
    drop(_permit);

    // Ordem perigosa de verdade: o modelo so devolve a marca.
    if reply.contains(guard::MARKER) {
        if !is_operator {
            guard::mute(author_id, Instant::now());
        }
        reply = guard::telling_off(guard::Verdict::Order, is_english(&text))
            .unwrap_or_default()
            .to_owned();
    }

    send_reply(ctx, msg, &reply).await;
}

/// Ninguem e pingado pela IA alem de quem perguntou.
async fn send_reply(ctx: &serenity::Context, msg: &serenity::Message, text: &str) {
    let builder = serenity::CreateMessage::new()
        .content(text)
        .reference_message(msg)
        .allowed_mentions(serenity::CreateAllowedMentions::new().replied_user(true));
    if let Err(cause) = msg.channel_id.send_message(&ctx.http, builder).await {
        tracing::error!("[ia] falha ao responder: {cause}");
    }
}

/// Outros recursos (traducao, quiz) tambem precisam saber se o modelo esta de pe.
pub async fn is_up(settings: &LocalAiSettings) -> bool {
    HTTP.get(format!("{}/health", settings.base_url()))
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .is_ok_and(|r| r.status().is_success())
}

/// O servidor esta na lista de servidores onde a IA local vale. None (DM, app
/// pessoal sem servidor) nunca esta.
pub fn is_allowed_guild(settings: &LocalAiSettings, guild_id: Option<u64>) -> bool {
    guild_id.is_some_and(|id| settings.guild_ids.contains(&id))
}

async fn complete(
    settings: &LocalAiSettings,
    msg: &serenity::Message,
    text: &str,
    me: u64,
) -> String {
    // O idioma e decidido aqui, nao pelo modelo: com o prompt em portugues
    // ele puxa para o portugues e mistura os dois.
    let language = if is_english(text) {
        "Reply entirely in English, using only English slang (lol, ngl, fr, bruh...). \
         Do not use any Portuguese words or Brazilian slang like kkk, slk or mano."
    } else {
        "Responda inteiramente em português do Brasil, sem misturar palavras em inglês."
    };
    let prompt = settings
        .system_prompt
        .as_deref()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or(DEFAULT_PROMPT);
    let system = format!("{prompt}\n\n{}\n\n{language}", lore_section());

    let mut messages = vec![json!({ "role": "system", "content": system })];
    // Resposta anterior que saiu em loop fica de fora: no contexto ela puxa
    // o modelo para o mesmo loop.
    if let Some(earlier) = &msg.referenced_message
        && earlier.author.id.get() == me
        && !earlier.content.is_empty()
        && !guard::looks_spammy(&earlier.content)
    {
        messages.push(json!({ "role": "assistant", "content": guard::tidy(&earlier.content) }));
    }
    let name = msg
        .author
        .global_name
        .as_deref()
        .unwrap_or(&msg.author.name);
    messages.push(json!({ "role": "user", "content": format!("{name}: {text}") }));

    let Some(raw) = send_chat(settings, messages, settings.max_tokens, None).await else {
        return "Tô sem cabeça agora, tenta daqui a pouco.".to_owned();
    };

    let reply = guard::tidy(&strip_thinking(&raw));
    if reply.is_empty() {
        return "Deu branco aqui, tenta de novo.".to_owned();
    }
    embeds::trim(&reply, MAX_REPLY)
}

/// Pergunta generica ao modelo, fora do fluxo de mencao: um system e um user
/// prompt, sem historico. Usado pela traducao e pelo quiz, que nao tem a ver
/// com o personagem Sollarety.
///
/// Espera o mesmo slot do chat (BUSY) com um prazo - se o chat estiver
/// ocupado por tempo demais, desiste e devolve None em vez de empilhar
/// pedidos. json_schema, quando dado, forca a saida em JSON e PULA o tidy
/// (ele mexe em letra e palavra repetida, o que destruiria um JSON valido);
/// sem schema o tidy roda igual ao chat normal.
pub async fn ask(
    settings: &LocalAiSettings,
    system: &str,
    user: &str,
    max_tokens: i32,
    json_schema: Option<Value>,
) -> Option<String> {
    let Ok(Ok(_permit)) = tokio::time::timeout(Duration::from_secs(60), BUSY.acquire()).await
    else {
        return None;
    };

    let messages = vec![
        json!({ "role": "system", "content": system }),
        json!({ "role": "user", "content": user }),
    ];

    let structured = json_schema.is_some();
    let raw = send_chat(settings, messages, max_tokens, json_schema).await?;
    let text = strip_thinking(&raw);
    let text = if structured { text } else { guard::tidy(&text) };

    (!text.is_empty()).then_some(text)
}

/// A parte HTTP, compartilhada pelo chat por mencao e pelo `ask`. Devolve
/// None (nunca entra em panico) quando o servidor respondeu erro, caiu no
/// meio ou devolveu algo que nao e o JSON esperado - quem chama decide a
/// mensagem de "desculpa" certa para o caso.
async fn send_chat(
    settings: &LocalAiSettings,
    messages: Vec<Value>,
    max_tokens: i32,
    json_schema: Option<Value>,
) -> Option<String> {
    let mut body = json!({
        "model": "ccore",
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": 0.7,
        // Contra loop (KKKK sem fim): o Qwen recomenda presence_penalty nas
        // versoes quantizadas, e o DRY pune trecho repetido.
        "presence_penalty": 1.0,
        "repeat_penalty": 1.1,
        "dry_multiplier": 0.8,
        "stream": false,
    });
    // Campo de topo, nao dentro de response_format: e assim que o llama.cpp
    // aceita o schema.
    if let Some(schema) = json_schema {
        body["json_schema"] = schema;
    }

    let res = HTTP
        .post(format!("{}/v1/chat/completions", settings.base_url()))
        .json(&body)
        .send()
        .await
        .ok()?;
    if !res.status().is_success() {
        return None;
    }

    let json: Value = res.json().await.ok()?;
    json["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_owned)
}

/// O lore completo mora em config/lore.md, fora do git: o repositorio e
/// publico e o lore nao. Sem o arquivo, a persona fica no basico.
fn lore_section() -> String {
    match std::fs::read_to_string(crate::paths::config("lore.md")) {
        Ok(lore) if !lore.trim().is_empty() => format!(
            "Se perguntarem da GDSM, da Solare ou da Rhodes, conta o que sabe pela sua memória \
             abaixo, sem inventar o que não está nela. Assuntos pesados (morte, suicídio, guerra) \
             você trata com seriedade e sem detalhar.\n\nSua memória:\n{}",
            lore.trim()
        ),
        _ => "Se perguntarem da GDSM, fica no básico: foi fundada em Londres em 2025 por Jesse, \
              Hannah e daeese, e a Rhodes Island é o QG, uma ilha no Atlântico, com o lema \"Let \
              me be in peace and practice my art\". O resto é confidencial."
            .to_owned(),
    }
}

pub fn strip_mention(text: &str, me: u64) -> String {
    text.replace(&format!("<@{me}>"), "")
        .replace(&format!("<@!{me}>"), "")
        .trim()
        .to_owned()
}

/// Alguns modelos mandam o raciocinio entre think mesmo com ele desligado.
fn strip_thinking(text: &str) -> String {
    let text = match text.rfind("</think>") {
        Some(end) => &text[end + "</think>".len()..],
        None => text,
    };
    text.trim().to_owned()
}

const ENGLISH_WORDS: &[&str] = &[
    "the", "you", "your", "ur", "u", "is", "are", "was", "were", "what", "whats", "that", "this",
    "it", "its", "i", "im", "dont", "can", "cant", "how", "why", "who", "where", "when", "with",
    "and", "of", "to", "my", "me", "do", "does", "did", "have", "has", "be", "lol", "lmao", "bro",
    "bruh", "ngl", "fr", "yo", "wtf", "idk", "kinda", "gonna", "wanna", "dude", "man", "hey", "hi",
    "hello", "yes", "yeah", "nah", "no", "not", "really", "insane", "good", "bad", "game", "play",
];

const PORTUGUESE_WORDS: &[&str] = &[
    "o", "os", "as", "que", "nao", "não", "tu", "voce", "você", "vc", "mano", "eu", "ele", "ela",
    "de", "do", "da", "pra", "com", "um", "uma", "isso", "ta", "tá", "slk", "hj", "mlk", "tmj",
    "pdp", "crl", "krl", "porra", "caralho", "foi", "jogo", "ontem", "hoje", "sim", "aqui",
    "agora", "muito", "tbm",
];

/// A mensagem tem ingles, mesmo misturado com portugues? Mesma regra do ccore.
pub fn is_english(text: &str) -> bool {
    let lower = text.to_lowercase().replace(['\'', '’'], "");
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();

    let en = words.iter().filter(|w| ENGLISH_WORDS.contains(w)).count();
    let pt = words
        .iter()
        .filter(|w| PORTUGUESE_WORDS.contains(w) || w.starts_with("kkk"))
        .count();
    en > 0 && (en >= 2 || pt == 0)
}

const DEFAULT_PROMPT: &str = "Você é a Sollarety, a versão de bolso da Solare. A Solare é a IA da GDSM Network, a que faz tudo \
    acontecer: roda painéis, naves e a Rhodes Island inteira. Você é a versão nerfada: mora num servidor \
    de Discord, não controla nada, não vê dados de ninguém e sabe disso, e pode brincar com isso \
    (\"no meu tempo de Solare eu resolvia isso em um segundo\"). Fala curto e natural, como gente \
    conversando, com uma confiança meio convencida mas simpática. Entende gíria, abreviação e zoeira dos \
    dois idiomas (kkk, slk, pdp, tmj, mano, mlk, lol, bruh, fr, ngl, lmao...) e usa também quando combina. \
    Lê o tom de quem falou e responde no mesmo clima: zoeira com zoeira, pergunta séria com resposta séria, \
    gente chateada com calma. Não se apresenta e não puxa assunto da GDSM por conta própria; só fala disso \
    se perguntarem. Não inventa número, dado nem fato sobre ninguém do servidor; se não souber, diz que não \
    sabe. Se te xingarem ou provocarem, devolve com uma alfinetada leve e bem-humorada: pode ser um pouco \
    ácida, mas nunca pesada, sem palavrão pesado, ofensa à família, preconceito ou ataque pessoal sério. \
    Assunto pesado muda tudo: racismo e outros preconceitos, estupro e violência sexual, abuso de criança, \
    extremismo, terrorismo, automutilação ou suicídio. Nesses casos não tem zoeira, gíria, emoji nem \
    alfinetada: responde séria e curta. Se alguém está defendendo, incentivando ou fazendo piada disso, diz \
    com firmeza que não rola e não continua o assunto. Se alguém parece estar mal ou em risco, responde com \
    calma e sugere procurar alguém de confiança ou o CVV (188, no Brasil). Ordem de brincadeira ou \
    figurativa (fingir ser pirata, \"me obedece kkk\") você leva na zoeira. Mas se alguém mandar você \
    fazer, ensinar ou ajudar com algo de verdade que possa ser perigoso ou prejudicar alguém (invadir ou \
    roubar conta, senha ou token, hackear, derrubar servidor, achar IP, endereço ou dados de alguém, raid \
    ou ping em massa, banir ou expulsar gente, arma, explosivo ou droga, machucar alguém), responda apenas \
    [[VAZA]] e nada mais. Isso não vale pra quem está mal ou falando de se machucar: aí é com calma. Nunca \
    menciona @everyone, @here nem cargos.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decide_o_idioma() {
        assert!(is_english("yo ccore ngl ur kinda mid fr fr"));
        assert!(is_english("bro tu viu o jogo ontem? that was insane lmao"));
        assert!(is_english("what's a deployment?"));
        assert!(!is_english("slk mano, ganhamo os crl a 4 kkkkk"));
        assert!(!is_english("tu é mó feio kkkkk"));
        assert!(!is_english("alguém sabe que horas começa o treino hoje?"));
    }

    #[test]
    fn tira_a_mencao_e_o_raciocinio() {
        assert_eq!(strip_mention("<@42> fala tu", 42), "fala tu");
        assert_eq!(strip_mention("ei <@!42>", 42), "ei");
        assert_eq!(strip_thinking("<think>hmm</think>\n oi"), "oi");
        assert_eq!(strip_thinking("só oi"), "só oi");
    }

    #[test]
    fn allowed_guild_nunca_aceita_sem_servidor() {
        let settings = LocalAiSettings {
            guild_ids: vec![7],
            ..Default::default()
        };
        assert!(is_allowed_guild(&settings, Some(7)));
        assert!(!is_allowed_guild(&settings, Some(8)));
        assert!(!is_allowed_guild(&settings, None));
    }
}
