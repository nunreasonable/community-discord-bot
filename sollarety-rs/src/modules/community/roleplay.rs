//! /roleplay, no molde da Loritta: abraco, tapa, high five e companhia, com
//! GIF de anime do nekos.best e um botao "Return" para o alvo devolver
//! (Roleplay.cs, Services/Fun/RoleplayActions.cs, NekosBest.cs).
//!
//! Tudo o que o clique de "Return" precisa vai no proprio custom id - nada
//! fica em memoria, entao o botao continua valendo depois do bot reiniciar.

use std::time::Duration;

use poise::{CreateReply, serenity_prelude as serenity};
use serde::Deserialize;
use serenity::Mentionable;

use crate::{Context, Data, Error, embeds, scope};

pub const PREFIX: &str = "rp:";

struct Action {
    key: &'static str,
    category: &'static str,
    emoji: &'static str,
    verb: &'static str,
    color: serenity::Colour,
    friendly: bool,
}

// Nenhuma cor vermelha, nem nas acoes "agressivas": vermelho e reservado
// para erro e recusa no bot inteiro (ver embeds.rs).
const ACTIONS: &[Action] = &[
    Action {
        key: "hug",
        category: "hug",
        emoji: "🤗",
        verb: "hugged",
        color: serenity::Colour::new(0xFF8DE6),
        friendly: true,
    },
    Action {
        key: "kiss",
        category: "kiss",
        emoji: "😘",
        verb: "kissed",
        color: serenity::Colour::new(0xFF6FA8),
        friendly: true,
    },
    Action {
        key: "slap",
        category: "slap",
        emoji: "🖐️",
        verb: "slapped",
        color: serenity::Colour::new(0xF0A04B),
        friendly: false,
    },
    Action {
        key: "pat",
        category: "pat",
        emoji: "🤚",
        verb: "patted",
        color: serenity::Colour::new(0xFFD166),
        friendly: true,
    },
    Action {
        key: "highfive",
        category: "highfive",
        emoji: "✋",
        verb: "high-fived",
        color: serenity::Colour::new(0x57C4E5),
        friendly: true,
    },
    Action {
        key: "dance",
        category: "dance",
        emoji: "💃",
        verb: "danced with",
        color: serenity::Colour::new(0xB980F0),
        friendly: true,
    },
    Action {
        key: "attack",
        category: "punch",
        emoji: "👊",
        verb: "attacked",
        color: serenity::Colour::new(0xE8894A),
        friendly: false,
    },
    Action {
        key: "cuddle",
        category: "cuddle",
        emoji: "🥰",
        verb: "cuddled",
        color: serenity::Colour::new(0xF7A1C4),
        friendly: true,
    },
    Action {
        key: "poke",
        category: "poke",
        emoji: "👉",
        verb: "poked",
        color: serenity::Colour::new(0x8BD3DD),
        friendly: true,
    },
    Action {
        key: "bonk",
        category: "bonk",
        emoji: "🔨",
        verb: "bonked",
        color: serenity::Colour::new(0xC9A66B),
        friendly: false,
    },
];

fn find_action(key: &str) -> Option<&'static Action> {
    ACTIONS.iter().find(|a| a.key == key)
}

/// Quem faz o que em quem, depois dos casos especiais. `None` no ator quer
/// dizer que a acao foi recusada (beijo no bot).
fn resolve(
    action: &Action,
    user: serenity::User,
    target: serenity::User,
    bot: serenity::User,
) -> (Option<serenity::User>, serenity::User, Option<&'static str>) {
    if target.id == user.id {
        // Abraco em si mesmo vira abraco do bot - como na Loritta.
        return if action.friendly {
            (Some(bot), user, Some("Everyone needs one sometimes."))
        } else {
            (Some(user.clone()), user, Some("Are you okay?"))
        };
    }

    if target.id == bot.id {
        if action.key == "kiss" {
            return (None, target, None);
        }
        // Bater no bot volta para quem bateu.
        return if action.friendly {
            (Some(user), bot, Some("Thanks! 💖"))
        } else {
            (Some(bot), user, Some("Nice try."))
        };
    }

    (Some(user), target, None)
}

fn sentence(
    action: &Action,
    actor: &serenity::User,
    target: &serenity::User,
    combo: i32,
) -> String {
    let who = if actor.id == target.id {
        "themselves".to_owned()
    } else {
        format!("<@{}>", target.id)
    };
    let combo_tag = if combo >= 3 {
        format!("**[COMBO x{combo}]** ")
    } else {
        String::new()
    };
    format!(
        "{combo_tag}{} <@{}> {} {who}!",
        action.emoji, actor.id, action.verb
    )
}

/// O cartao completo, com o botao "Return" quando faz sentido: o alvo
/// precisa ser uma pessoa e outra que nao quem agiu.
async fn build(
    http: &reqwest::Client,
    action: &Action,
    actor: &serenity::User,
    target: &serenity::User,
    combo: i32,
    note: Option<&str>,
) -> (serenity::CreateEmbed, Vec<serenity::CreateButton>) {
    let gif = fetch_gif(http, action.category).await;

    let mut description = sentence(action, actor, target, combo);
    if let Some(note) = note {
        description.push_str(&format!("\n*{note}*"));
    }

    let mut embed = serenity::CreateEmbed::new()
        .description(description)
        .colour(action.color);

    embed = match gif {
        Some(gif) => {
            embed = embed.image(gif.url);
            embed.footer(serenity::CreateEmbedFooter::new(match gif.anime_name {
                Some(name) => format!("Anime: {} · via nekos.best", embeds::trim(&name, 100)),
                None => "via nekos.best".to_owned(),
            }))
        }
        None => embed.footer(serenity::CreateEmbedFooter::new(
            "Couldn't fetch a GIF this time.",
        )),
    };

    let buttons = if target.bot || target.id == actor.id {
        Vec::new()
    } else {
        vec![return_button(
            action,
            actor.id.get(),
            target.id.get(),
            combo,
            true,
        )]
    };

    (embed, buttons)
}

/// "rp:acao:de:para:combo".
fn return_button(
    action: &Action,
    from_id: u64,
    to_id: u64,
    combo: i32,
    enabled: bool,
) -> serenity::CreateButton {
    serenity::CreateButton::new(format!("{PREFIX}{}:{from_id}:{to_id}:{combo}", action.key))
        .style(serenity::ButtonStyle::Primary)
        .label("Return")
        .emoji(serenity::ReactionType::Unicode(action.emoji.into()))
        .disabled(!enabled)
}

// ------------------------------------------------------------------
// nekos.best
// ------------------------------------------------------------------

struct Gif {
    url: String,
    anime_name: Option<String>,
}

#[derive(Deserialize)]
struct NekosResponse {
    results: Vec<NekosResult>,
}

#[derive(Deserialize)]
struct NekosResult {
    url: String,
    anime_name: Option<String>,
}

const NEKOS_TIMEOUT: Duration = Duration::from_secs(8);

/// Um GIF da categoria, ou None se a API falhar - o comando sai sem imagem,
/// mas sai. A categoria vem sempre da tabela de acoes, nunca do usuario.
async fn fetch_gif(http: &reqwest::Client, category: &str) -> Option<Gif> {
    let url = format!("https://nekos.best/api/v2/{category}");
    let response = match http.get(&url).timeout(NEKOS_TIMEOUT).send().await {
        Ok(response) => response,
        Err(cause) => {
            tracing::warn!("[roleplay] aviso: nekos.best falhou para '{category}': {cause}");
            return None;
        }
    };

    if !response.status().is_success() {
        tracing::warn!(
            "[roleplay] aviso: nekos.best respondeu {} para '{category}'",
            response.status()
        );
        return None;
    }

    let body: NekosResponse = match response.json().await {
        Ok(body) => body,
        Err(cause) => {
            tracing::warn!(
                "[roleplay] aviso: nekos.best devolveu algo inesperado para '{category}': {cause}"
            );
            return None;
        }
    };
    let first = body.results.into_iter().next()?;

    // So https e so do proprio nekos.best: uma resposta estranha nao pode
    // virar imagem num cartao assinado pelo bot.
    let Ok(parsed) = url::Url::parse(&first.url) else {
        tracing::warn!(
            "[roleplay] aviso: nekos.best devolveu uma URL inesperada para '{category}'"
        );
        return None;
    };
    let host_ok =
        matches!(parsed.host_str(), Some(h) if h == "nekos.best" || h.ends_with(".nekos.best"));
    if parsed.scheme() != "https" || !host_ok {
        tracing::warn!(
            "[roleplay] aviso: nekos.best devolveu uma URL inesperada para '{category}'"
        );
        return None;
    }

    Some(Gif {
        url: parsed.into(),
        anime_name: first.anime_name.filter(|n| !n.trim().is_empty()),
    })
}

// ------------------------------------------------------------------
// Comandos
// ------------------------------------------------------------------

macro_rules! roleplay_command {
    ($fn_name:ident, $rename:literal, $desc:literal, $user_desc:literal, $key:literal) => {
        #[doc = $desc]
        #[poise::command(slash_command, rename = $rename, check = "crate::cooldown::user::<3, 10>")]
        pub async fn $fn_name(
            ctx: Context<'_>,
            #[description = $user_desc] user: serenity::User,
        ) -> Result<(), Error> {
            run(ctx, $key, user).await
        }
    };
}

roleplay_command!(hug, "hug", "Give someone a hug", "Who to hug", "hug");
roleplay_command!(kiss, "kiss", "Give someone a kiss", "Who to kiss", "kiss");
roleplay_command!(slap, "slap", "Slap someone", "Who to slap", "slap");
roleplay_command!(pat, "pat", "Give someone a headpat", "Who to pat", "pat");
roleplay_command!(
    highfive,
    "highfive",
    "High-five someone",
    "Who to high-five",
    "highfive"
);
roleplay_command!(
    dance,
    "dance",
    "Dance with someone",
    "Who to dance with",
    "dance"
);
roleplay_command!(
    attack,
    "attack",
    "Attack someone",
    "Who to attack",
    "attack"
);
roleplay_command!(
    cuddle,
    "cuddle",
    "Cuddle with someone",
    "Who to cuddle",
    "cuddle"
);
roleplay_command!(poke, "poke", "Poke someone", "Who to poke", "poke");
roleplay_command!(bonk, "bonk", "Bonk someone", "Who to bonk", "bonk");

async fn run(ctx: Context<'_>, key: &str, user: serenity::User) -> Result<(), Error> {
    let action = find_action(key).expect("tabela de acoes cobre todo comando");
    let bot: serenity::User = (*ctx.cache().current_user()).clone().into();
    let (actor, target, note) = resolve(action, ctx.author().clone(), user, bot);

    let Some(actor) = actor else {
        let embed = serenity::CreateEmbed::new()
            .description(format!(
                "😳 {}, I'm flattered, but I'm just a bot. How about a hug instead?",
                ctx.author().mention()
            ))
            .colour(action.color);
        ctx.send(CreateReply::default().embed(embed)).await?;
        return Ok(());
    };

    // O GIF vem de fora e pode levar alguns segundos.
    ctx.defer().await?;

    let (embed, buttons) = build(&ctx.data().http, action, &actor, &target, 1, note).await;
    let mut reply = CreateReply::default().embed(embed);
    if !buttons.is_empty() {
        reply = reply.components(vec![serenity::CreateActionRow::Buttons(buttons)]);
    }
    ctx.send(reply).await?;
    Ok(())
}

// ------------------------------------------------------------------
// Botao "rp:" (Return)
// ------------------------------------------------------------------

pub async fn on_component(
    ctx: &serenity::Context,
    data: &Data,
    press: &serenity::ComponentInteraction,
) -> Result<(), Error> {
    let id = press.data.custom_id.as_str();
    if !id.starts_with(PREFIX) {
        return Ok(());
    }

    let parts: Vec<&str> = id.split(':').collect();
    let parsed = (|| -> Option<(&'static Action, u64, u64, i32)> {
        if parts.len() != 5 {
            return None;
        }
        let action = find_action(parts[1])?;
        let from_id: u64 = parts[2].parse().ok()?;
        let to_id: u64 = parts[3].parse().ok()?;
        let combo: i32 = parts[4].parse().ok()?;
        Some((action, from_id, to_id, combo))
    })();

    let Some((action, from_id, to_id, combo)) = parsed else {
        tracing::warn!("[fun] custom id de roleplay fora do formato: '{id}'");
        press
            .create_response(
                ctx,
                serenity::CreateInteractionResponse::Message(
                    serenity::CreateInteractionResponseMessage::new()
                        .embed(embeds::error(
                            "Error",
                            "This button is from an older version of the bot.",
                        ))
                        .ephemeral(true),
                ),
            )
            .await?;
        return Ok(());
    };

    if press.user.id.get() != to_id {
        press
            .create_response(
                ctx,
                serenity::CreateInteractionResponse::Message(
                    serenity::CreateInteractionResponseMessage::new()
                        .embed(embeds::info(
                            "Not for you",
                            format!(
                                "Only <@{to_id}> can return this. Start your own with `/roleplay`."
                            ),
                        ))
                        .ephemeral(true),
                ),
            )
            .await?;
        return Ok(());
    }

    // Desliga o botao ANTES de responder com o cartao novo: sem isso o
    // mesmo "Return" podia ser clicado de novo e o combo andava sozinho.
    let original_embeds: Vec<serenity::CreateEmbed> = press
        .message
        .embeds
        .iter()
        .map(|e| serenity::CreateEmbed::from(e.clone()))
        .collect();
    press
        .create_response(
            ctx,
            serenity::CreateInteractionResponse::UpdateMessage(
                serenity::CreateInteractionResponseMessage::new()
                    .embeds(original_embeds)
                    .components(vec![serenity::CreateActionRow::Buttons(vec![
                        return_button(action, from_id, to_id, combo, false),
                    ])]),
            ),
        )
        .await?;

    let from = match serenity::UserId::new(from_id).to_user(ctx).await {
        Ok(user) => user,
        Err(cause) => {
            tracing::warn!(
                "[fun] falha ao buscar o usuario {from_id} do botao de roleplay: {cause}"
            );
            return Ok(());
        }
    };
    let bot: serenity::User = (*ctx.cache.current_user()).clone().into();
    let (actor, target, note) = resolve(action, press.user.clone(), from, bot);

    let Some(actor) = actor else {
        press
            .create_followup(
                ctx,
                serenity::CreateInteractionResponseFollowup::new().embed(
                    serenity::CreateEmbed::new()
                        .description(format!(
                            "😳 {}, I'm flattered, but I'm just a bot. How about a hug instead?",
                            press.user.mention()
                        ))
                        .colour(action.color),
                ),
            )
            .await?;
        return Ok(());
    };

    let (embed, buttons) = build(
        &data.http,
        action,
        &actor,
        &target,
        (combo + 1).min(999),
        note,
    )
    .await;
    let mut followup = serenity::CreateInteractionResponseFollowup::new().embed(embed);
    if !buttons.is_empty() {
        followup = followup.components(vec![serenity::CreateActionRow::Buttons(buttons)]);
    }
    press.create_followup(ctx, followup).await?;
    Ok(())
}

/// Grupo /roleplay com os dez subcomandos.
#[poise::command(
    slash_command,
    rename = "roleplay",
    subcommands(
        "hug", "kiss", "slap", "pat", "highfive", "dance", "attack", "cuddle", "poke", "bonk"
    ),
    subcommand_required
)]
pub async fn roleplay_root(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

pub fn commands() -> Vec<poise::Command<Data, Error>> {
    vec![scope::anywhere(roleplay_root())]
}
