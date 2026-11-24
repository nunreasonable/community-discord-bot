//! Recursos que usam a IA local fora do chat por mencao: o menu de contexto
//! "Translate" numa mensagem, e o /quiz (commands/Ai.cs). Os dois pedem ao
//! modelo via `local::ask` - nao tem a ver com a persona Sollarety do
//! `local::on_message_create`, so o servidor por tras e o mesmo.

use poise::serenity_prelude as serenity;
use serenity::Mentionable;

use super::{local, quiz};
use crate::{Context, Error, embeds, store};

const MIN_QUESTIONS: i64 = 3;
const MAX_QUESTIONS: i64 = 5;

// ----------------------------------------------------------------------
// Menu de contexto: Translate
// ----------------------------------------------------------------------

/// Translate a message to Portuguese or English using the local AI
#[poise::command(
    context_menu_command = "Translate",
    slash_command,
    check = "crate::cooldown::user::<3, 20>"
)]
pub async fn translate(ctx: Context<'_>, msg: serenity::Message) -> Result<(), Error> {
    let settings = ctx.data().config().local_ai.clone();

    if !local::is_allowed_guild(&settings, ctx.guild_id().map(|g| g.get())) {
        ctx.send(
            poise::CreateReply::default()
                .embed(embeds::info(
                    "Not available here",
                    "This only works in servers where the local AI is turned on.",
                ))
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    // Sem o intent de Message Content para a maioria das mensagens, mas o
    // Discord manda o conteudo da mensagem-alvo de um menu de contexto mesmo
    // assim - e por isso que isto funciona sem pedir o intent.
    let text = msg.content.trim();
    if text.is_empty() {
        ctx.send(
            poise::CreateReply::default()
                .embed(embeds::error(
                    "Nothing to translate",
                    "That message doesn't have any text — just an embed, image or attachment, maybe.",
                ))
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    if !local::is_up(&settings).await {
        ctx.send(
            poise::CreateReply::default()
                .embed(embeds::info(
                    "AI is off",
                    "The local AI isn't running right now. Try again later.",
                ))
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    // Pode levar de 5 a 30s: o modelo so tem um slot, e pode estar ocupado
    // com outra coisa.
    ctx.defer_ephemeral().await?;

    let to_portuguese = local::is_english(text);
    let system = if to_portuguese {
        "You are a translation engine. Translate the user's message into natural Brazilian \
         Portuguese. Output ONLY the translation and nothing else: no quotes, no notes, no \
         explanations. Keep the same tone, and keep mentions, markdown and links exactly as they \
         are."
    } else {
        "You are a translation engine. Translate the user's message into natural English. Output \
         ONLY the translation and nothing else: no quotes, no notes, no explanations. Keep the same \
         tone, and keep mentions, markdown and links exactly as they are."
    };

    let translated = local::ask(&settings, system, text, 500, None).await;

    match translated.filter(|t| !t.trim().is_empty()) {
        None => {
            ctx.send(poise::CreateReply::default().embed(embeds::error(
                "Couldn't translate that",
                "Something went wrong talking to the AI. Try again later.",
            )))
            .await?;
        }
        // So a traducao, sem embed - e tudo que o pedido quer de volta.
        Some(translated) => {
            ctx.send(poise::CreateReply::default().content(embeds::trim(&translated, 1900)))
                .await?;
        }
    }

    Ok(())
}

// ----------------------------------------------------------------------
// /quiz
// ----------------------------------------------------------------------

/// Starts a themed trivia quiz for this channel
#[poise::command(slash_command, check = "crate::cooldown::user::<1, 10>")]
pub async fn quiz(
    ctx: Context<'_>,
    #[description = "What the quiz should be about"] theme: String,
    #[description = "How many questions (3 to 5, default 3)"]
    #[min = 3]
    #[max = 5]
    questions: Option<i64>,
) -> Result<(), Error> {
    // guild_only (scope::guild_only) garante o servidor; sem ele o comando
    // nem aparece fora de um.
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };

    let settings = ctx.data().config().local_ai.clone();

    if !local::is_allowed_guild(&settings, Some(guild_id.get())) {
        return refuse(
            ctx,
            embeds::info(
                "Not available here",
                "The local AI quiz is off in this server.",
            ),
        )
        .await;
    }

    if !local::is_up(&settings).await {
        return refuse(
            ctx,
            embeds::info(
                "AI is off",
                "The local AI isn't running right now. Try again later.",
            ),
        )
        .await;
    }

    let channel_id = ctx.channel_id();
    let game = match quiz::try_reserve(channel_id.get()) {
        quiz::QuizReserve::Active => {
            return refuse(
                ctx,
                embeds::info(
                    "Quiz already running",
                    "There's already a quiz going in this channel. Wait for it to finish.",
                ),
            )
            .await;
        }
        quiz::QuizReserve::Cooldown(until) => {
            return refuse(
                ctx,
                embeds::info(
                    "Still cooling down",
                    format!(
                        "This channel just had a quiz. Try again <t:{}:R>.",
                        until.timestamp()
                    ),
                ),
            )
            .await;
        }
        quiz::QuizReserve::Ok(game) => game,
    };

    // Perguntas e 3 a 5 pelo min/max do parametro, mas a geracao e cara o
    // bastante para valer uma segunda trava aqui.
    let count = questions
        .unwrap_or(MIN_QUESTIONS)
        .clamp(MIN_QUESTIONS, MAX_QUESTIONS) as usize;

    // Publico, e nao efemero: o quiz e pra o canal inteiro jogar, e pode
    // levar de 5 a 30s para a IA montar as perguntas.
    ctx.defer().await?;

    let Some(generated) = quiz::generate(&settings, &theme, count).await else {
        quiz::cancel(&game);
        ctx.send(poise::CreateReply::default().embed(embeds::error(
            "Couldn't build that quiz",
            "The AI didn't come back with a usable set of questions for that theme. Try again, maybe \
             with a different theme.",
        )))
        .await?;
        return Ok(());
    };

    let paid = ctx
        .data()
        .guild_settings
        .for_guild(guild_id.get())
        .is_some_and(|s| s.economy_enabled);
    game.set_questions(generated, paid);
    let total = game.question_count();

    let reward_text = if paid {
        format!(
            "First to answer each one right gets {}.",
            store::economy::format(quiz::REWARD)
        )
    } else {
        "First to answer each one right gets the point.".to_owned()
    };
    ctx.send(
        poise::CreateReply::default()
            .embed(
                serenity::CreateEmbed::new()
                    .title("🧠 Quiz time!")
                    .description(format!(
                        "{} started a quiz about **{}** — {total} question(s). {reward_text}",
                        ctx.author().mention(),
                        embeds::safe(&theme)
                    ))
                    .colour(embeds::colors::BLURPLE),
            )
            .allowed_mentions(serenity::CreateAllowedMentions::new()),
    )
    .await?;

    // Daqui pra frente e o /quiz tocando sozinho: a interacao ja foi
    // respondida, e o resto sao mensagens novas no canal.
    let http = ctx.serenity_context().http.clone();
    let data = ctx.data().clone();
    tokio::spawn(quiz::run(http, channel_id, game, data));

    Ok(())
}

async fn refuse(ctx: Context<'_>, embed: serenity::CreateEmbed) -> Result<(), Error> {
    ctx.send(poise::CreateReply::default().embed(embed).ephemeral(true))
        .await?;
    Ok(())
}
