//! Economia de SOL$, no molde da Loritta: /daily, /work, /pay (com
//! confirmacao pelos botoes "eco:") e /balance (Economy.cs,
//! Services/Economy/EconomyService.cs, EconomyComponents.cs).
//!
//! A carteira e GLOBAL (a mesma em todo servidor), mas os comandos so
//! funcionam onde quem tem Manage Server ligou /config economy.

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
};

use chrono::{DateTime, Days, TimeZone, Utc};
use poise::{CreateReply, serenity_prelude as serenity};
use serenity::Mentionable;

use crate::{
    Context, Data, Error, embeds, scope,
    store::{self, EconomyStore, Wallet},
};

const PREFIX: &str = "eco:";
/// Cor da Economy.cs (0xF2B705), separada do dourado dos niveis.
const COLOR: serenity::Colour = serenity::Colour::new(0xF2B705);

pub const DAILY_BASE: i64 = 500;
pub const DAILY_STREAK_BONUS: i64 = 50;
pub const DAILY_STREAK_CAP: i32 = 10;

pub const WORK_COOLDOWN_SECS: i64 = 60 * 60;
pub const WORK_MIN: i64 = 80;
pub const WORK_MAX: i64 = 250;

/// Duas horas, como `EconomyService.PayConfirmWindow`.
const PAY_CONFIRM_WINDOW_SECS: i64 = 2 * 60;

fn economy_enabled(data: &Data, guild_id: u64) -> bool {
    data.guild_settings
        .for_guild(guild_id)
        .map(|s| s.economy_enabled)
        .unwrap_or(false)
}

fn economy_off() -> serenity::CreateEmbed {
    embeds::error(
        "The economy is off in this server",
        "Someone with **Manage Server** can turn it on with `/config economy`.",
    )
}

fn when(at: DateTime<Utc>) -> String {
    format!("<t:{}:R>", at.timestamp())
}

async fn refuse(ctx: Context<'_>, embed: serenity::CreateEmbed) -> Result<(), Error> {
    ctx.send(CreateReply::default().embed(embed).ephemeral(true))
        .await?;
    Ok(())
}

// ------------------------------------------------------------------
// Daily
// ------------------------------------------------------------------

pub struct DailyResult {
    pub claimed: bool,
    pub reward: i64,
    pub streak: i32,
    pub next_at: DateTime<Utc>,
    pub balance: i64,
}

/// Um claim por dia UTC. A sequencia continua se o ultimo foi ontem e volta
/// a 1 se nao foi; cada dia seguido soma 50 ao premio, ate 10 dias (500 a
/// 1.000 SOL$).
pub async fn claim_daily(
    store: &EconomyStore,
    user_id: u64,
    now: DateTime<Utc>,
) -> Result<DailyResult, Error> {
    store
        .update(|edit| {
            let wallet = edit.get(user_id);
            let today = now.date_naive();
            let last = wallet.last_daily_utc.map(|t| t.date_naive());
            let next = tomorrow_midnight_utc(today);

            if last == Some(today) {
                return DailyResult {
                    claimed: false,
                    reward: 0,
                    streak: wallet.daily_streak,
                    next_at: next,
                    balance: wallet.balance,
                };
            }

            let streak = if last == today.checked_sub_days(Days::new(1)) {
                wallet.daily_streak + 1
            } else {
                1
            };
            let reward =
                DAILY_BASE + DAILY_STREAK_BONUS * i64::from((streak - 1).min(DAILY_STREAK_CAP));

            wallet.balance += reward;
            wallet.daily_streak = streak;
            wallet.last_daily_utc = Some(now);
            let balance = wallet.balance;
            edit.mark_changed();

            DailyResult {
                claimed: true,
                reward,
                streak,
                next_at: next,
                balance,
            }
        })
        .await
}

fn tomorrow_midnight_utc(today: chrono::NaiveDate) -> DateTime<Utc> {
    let tomorrow = today.succ_opt().unwrap_or(today);
    Utc.from_utc_datetime(&tomorrow.and_hms_opt(0, 0, 0).unwrap_or_default())
}

/// Quando o proximo daily libera, ou None se ja esta liberado.
pub fn next_daily(wallet: Option<&Wallet>, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let today = now.date_naive();
    (wallet
        .and_then(|w| w.last_daily_utc)
        .map(|t| t.date_naive())
        == Some(today))
    .then(|| tomorrow_midnight_utc(today))
}

/// A sequencia que ainda vale: quem pulou um dia ja a perdeu, mesmo antes do
/// proximo claim.
pub fn live_streak(wallet: Option<&Wallet>, now: DateTime<Utc>) -> i32 {
    let Some(wallet) = wallet else { return 0 };
    let Some(last) = wallet.last_daily_utc.map(|t| t.date_naive()) else {
        return 0;
    };
    let today = now.date_naive();
    if last == today || last == today.checked_sub_days(Days::new(1)).unwrap_or(today) {
        wallet.daily_streak
    } else {
        0
    }
}

// ------------------------------------------------------------------
// Work
// ------------------------------------------------------------------

const JOBS: &[&str] = &[
    "You walked a pack of very excited dogs around the block",
    "You delivered pizzas across town (only one got slightly squished)",
    "You fixed a printer that was definitely haunted",
    "You streamed for three hours to an audience of four",
    "You debugged someone else's code with zero comments",
    "You moderated a Discord server through a meme war",
    "You worked the night shift at the space station canteen",
    "You taught a parrot to say \"git push --force\"",
    "You sorted 2,000 Lego bricks by color",
    "You photographed a very serious corgi wedding",
    "You harvested solar panels on the sunny side of the server",
    "You translated a menu written entirely in emoji",
    "You tuned the pianos at the local concert hall",
    "You beta-tested a game that crashed on the title screen",
    "You returned 47 overdue library books for the whole town",
];

pub struct WorkResult {
    pub worked: bool,
    pub reward: i64,
    pub job: String,
    pub next_at: DateTime<Utc>,
    pub balance: i64,
}

pub async fn work(
    store: &EconomyStore,
    user_id: u64,
    now: DateTime<Utc>,
) -> Result<WorkResult, Error> {
    store
        .update(|edit| {
            let wallet = edit.get(user_id);

            if let Some(last) = wallet.last_work_utc
                && (now - last).num_seconds() < WORK_COOLDOWN_SECS
            {
                return WorkResult {
                    worked: false,
                    reward: 0,
                    job: String::new(),
                    next_at: last + chrono::Duration::seconds(WORK_COOLDOWN_SECS),
                    balance: wallet.balance,
                };
            }

            let mut rng = rand::rng();
            let reward = rand::Rng::random_range(&mut rng, WORK_MIN..=WORK_MAX);
            let job = JOBS[rand::Rng::random_range(&mut rng, 0..JOBS.len())];

            wallet.balance += reward;
            wallet.last_work_utc = Some(now);
            let balance = wallet.balance;
            edit.mark_changed();

            WorkResult {
                worked: true,
                reward,
                job: job.to_owned(),
                next_at: now + chrono::Duration::seconds(WORK_COOLDOWN_SECS),
                balance,
            }
        })
        .await
}

// ------------------------------------------------------------------
// Pay
// ------------------------------------------------------------------

pub struct TransferResult {
    pub done: bool,
    pub refusal: Option<String>,
    pub payer_balance: i64,
}

/// Debita e credita na MESMA gravacao: nao existe meio pagamento.
pub async fn transfer(
    store: &EconomyStore,
    payer_id: u64,
    target_id: u64,
    amount: i64,
) -> Result<TransferResult, Error> {
    store
        .update(|edit| {
            if amount <= 0 {
                return TransferResult {
                    done: false,
                    refusal: Some("The amount has to be more than zero.".to_owned()),
                    payer_balance: 0,
                };
            }
            if payer_id == target_id {
                return TransferResult {
                    done: false,
                    refusal: Some("You can't pay yourself.".to_owned()),
                    payer_balance: 0,
                };
            }

            let payer_balance = edit.get(payer_id).balance;
            if payer_balance < amount {
                return TransferResult {
                    done: false,
                    refusal: Some(format!(
                        "You only have {}.",
                        store::economy::format(payer_balance)
                    )),
                    payer_balance,
                };
            }

            edit.get(payer_id).balance -= amount;
            edit.get(target_id).balance = edit.get(target_id).balance.saturating_add(amount);
            edit.mark_changed();

            TransferResult {
                done: true,
                refusal: None,
                payer_balance: edit.get(payer_id).balance,
            }
        })
        .await
}

struct PendingPay {
    payer_id: u64,
    target_id: u64,
    amount: i64,
    expires_at: DateTime<Utc>,
}

static PENDING: LazyLock<Mutex<HashMap<String, PendingPay>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Guarda o pedido e devolve a chave que vai no botao. O valor fica AQUI, e
/// nao no custom id: o botao so carrega uma chave aleatoria, que nao da para
/// forjar nem reaproveitar.
fn register_pending(payer_id: u64, target_id: u64, amount: i64, now: DateTime<Utc>) -> String {
    let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    pending.retain(|_, p| p.expires_at >= now);

    let nonce = store::short_id(16);
    pending.insert(
        nonce.clone(),
        PendingPay {
            payer_id,
            target_id,
            amount,
            expires_at: now + chrono::Duration::seconds(PAY_CONFIRM_WINDOW_SECS),
        },
    );
    nonce
}

enum TakePending {
    None,
    NotYours,
    Found(PendingPay),
}

/// Tira o pedido da fila (uso unico) se quem clicou e quem paga. Expirou,
/// nunca existiu ou ja foi usado: None - um reinicio do bot tambem zera os
/// pendentes.
fn take_pending(nonce: &str, clicker_id: u64, now: DateTime<Utc>) -> TakePending {
    let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    let Some(found) = pending.get(nonce) else {
        return TakePending::None;
    };
    if found.payer_id != clicker_id {
        return TakePending::NotYours;
    }
    let Some(found) = pending.remove(nonce) else {
        return TakePending::None;
    };
    if found.expires_at < now {
        return TakePending::None;
    }
    TakePending::Found(found)
}

// ------------------------------------------------------------------
// Comandos
// ------------------------------------------------------------------

/// Claim your daily SOL$ — come back every day to build a streak
#[poise::command(
    slash_command,
    rename = "daily",
    guild_only,
    check = "crate::cooldown::user::<3, 10>"
)]
pub async fn daily(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().expect("guild_only garante o servidor");
    if !economy_enabled(ctx.data(), guild_id.get()) {
        return refuse(ctx, economy_off()).await;
    }

    let now = Utc::now();
    let result = claim_daily(&ctx.data().economy, ctx.author().id.get(), now).await?;

    if !result.claimed {
        return refuse(
            ctx,
            embeds::info(
                "Already claimed today",
                format!(
                    "Your next daily unlocks {}. 🔥 Streak: **{}** day(s).",
                    when(result.next_at),
                    result.streak
                ),
            ),
        )
        .await;
    }

    let cap = DAILY_BASE + DAILY_STREAK_BONUS * i64::from(DAILY_STREAK_CAP);
    let bonus = if result.streak > 1 {
        format!(
            "\n🔥 **{}-day streak** — keep it going for a bigger reward (up to {}).",
            result.streak,
            store::economy::format(cap)
        )
    } else {
        "\nCome back tomorrow to start a streak.".to_owned()
    };

    let embed = serenity::CreateEmbed::new()
        .title("☀️ Daily claimed")
        .description(format!(
            "{} got **{}**!{bonus}\n\nBalance: **{}** · next daily {}",
            ctx.author().mention(),
            store::economy::format(result.reward),
            store::economy::format(result.balance),
            when(result.next_at)
        ))
        .colour(COLOR);

    ctx.send(
        CreateReply::default()
            .embed(embed)
            .allowed_mentions(serenity::CreateAllowedMentions::new()),
    )
    .await?;
    Ok(())
}

/// Work a shift for some SOL$ (once an hour)
#[poise::command(
    slash_command,
    rename = "work",
    guild_only,
    check = "crate::cooldown::user::<3, 10>"
)]
pub async fn work_command(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().expect("guild_only garante o servidor");
    if !economy_enabled(ctx.data(), guild_id.get()) {
        return refuse(ctx, economy_off()).await;
    }

    let now = Utc::now();
    let result = work(&ctx.data().economy, ctx.author().id.get(), now).await?;

    if !result.worked {
        return refuse(
            ctx,
            embeds::info(
                "You're still tired",
                format!("You can work again {}.", when(result.next_at)),
            ),
        )
        .await;
    }

    let embed = serenity::CreateEmbed::new()
        .title("💼 Shift done")
        .description(format!(
            "{} and earned **{}**.\n\nBalance: **{}** · next shift {}",
            result.job,
            store::economy::format(result.reward),
            store::economy::format(result.balance),
            when(result.next_at)
        ))
        .colour(COLOR);
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Send some of your SOL$ to someone
#[poise::command(
    slash_command,
    rename = "pay",
    guild_only,
    check = "crate::cooldown::user::<3, 15>"
)]
pub async fn pay(
    ctx: Context<'_>,
    #[description = "Who gets the SOL$"] user: serenity::User,
    #[description = "How many SOL$ to send"]
    #[min = 1]
    amount: i64,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().expect("guild_only garante o servidor");
    if !economy_enabled(ctx.data(), guild_id.get()) {
        return refuse(ctx, economy_off()).await;
    }

    // Recusas antes do pedido de confirmacao: nao faz sentido perguntar "tem
    // certeza?" de algo que nao vai poder acontecer.
    let balance = ctx
        .data()
        .economy
        .for_user(ctx.author().id.get())
        .map(|w| w.balance)
        .unwrap_or(0);

    let refusal = if user.bot {
        Some("Bots don't have wallets.".to_owned())
    } else if user.id == ctx.author().id {
        Some("You can't pay yourself.".to_owned())
    } else if amount <= 0 {
        Some("The amount has to be more than zero.".to_owned())
    } else if balance < amount {
        Some(format!(
            "You only have {}.",
            store::economy::format(balance)
        ))
    } else {
        None
    };

    if let Some(refusal) = refusal {
        return refuse(ctx, embeds::error("Can't send that", refusal)).await;
    }

    let now = Utc::now();
    let nonce = register_pending(ctx.author().id.get(), user.id.get(), amount, now);

    let embed = serenity::CreateEmbed::new()
        .title("💸 Confirm payment")
        .description(format!(
            "{}, send **{}** to {}?\n-# Only you can confirm. This expires {}.",
            ctx.author().mention(),
            store::economy::format(amount),
            user.mention(),
            when(now + chrono::Duration::seconds(PAY_CONFIRM_WINDOW_SECS))
        ))
        .colour(COLOR);

    let components = vec![serenity::CreateActionRow::Buttons(vec![
        serenity::CreateButton::new(format!("{PREFIX}pay:{nonce}"))
            .style(serenity::ButtonStyle::Success)
            .label("Send")
            .emoji(serenity::ReactionType::Unicode("✅".into())),
        serenity::CreateButton::new(format!("{PREFIX}cancel:{nonce}"))
            .style(serenity::ButtonStyle::Secondary)
            .label("Cancel"),
    ])];

    ctx.send(
        CreateReply::default()
            .embed(embed)
            .components(components)
            .allowed_mentions(serenity::CreateAllowedMentions::new()),
    )
    .await?;
    Ok(())
}

/// See how many SOL$ someone has
#[poise::command(
    slash_command,
    rename = "balance",
    guild_only,
    check = "crate::cooldown::user::<3, 10>"
)]
pub async fn balance(
    ctx: Context<'_>,
    #[description = "Whose wallet (default: you)"] user: Option<serenity::User>,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().expect("guild_only garante o servidor");
    if !economy_enabled(ctx.data(), guild_id.get()) {
        return refuse(ctx, economy_off()).await;
    }

    let target = user.unwrap_or_else(|| ctx.author().clone());
    let balance = ctx
        .data()
        .economy
        .for_user(target.id.get())
        .map(|w| w.balance)
        .unwrap_or(0);
    let board = ctx.data().economy.board();
    let position = board.iter().position(|&(u, _)| u == target.id.get());

    let name = super::common::display_name(ctx, Some(guild_id), &target).await;
    let mut author =
        serenity::CreateEmbedAuthor::new(embeds::trim(&format!("{name}'s wallet"), 256));
    if let Some(icon) = target.avatar_url() {
        author = author.icon_url(icon);
    }

    let mut description = format!("**{}**", store::economy::format(balance));
    if let Some(p) = position {
        description.push_str(&format!(
            "\nGlobal rank **#{}** of {}",
            p + 1,
            embeds::number(board.len() as i64)
        ));
    }

    let embed = serenity::CreateEmbed::new()
        .author(author)
        .description(description)
        .colour(COLOR);
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

// ------------------------------------------------------------------
// Botoes "eco:" (confirmacao do /pay)
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
    if parts.len() != 3 || !matches!(parts[1], "pay" | "cancel") {
        tracing::warn!("[economia] custom id fora do formato: '{id}'");
        ephemeral_press(
            ctx,
            press,
            embeds::error(
                "Unknown button",
                "This button is from an older version of the bot.",
            ),
        )
        .await?;
        return Ok(());
    }

    let now = Utc::now();
    match take_pending(parts[2], press.user.id.get(), now) {
        TakePending::NotYours => {
            ephemeral_press(
                ctx,
                press,
                embeds::info(
                    "Not your payment",
                    "Only the person sending the SOL$ can confirm it.",
                ),
            )
            .await?;
        }
        TakePending::None => {
            finish(
                ctx,
                press,
                embeds::info(
                    "Payment expired",
                    "This payment expired or was already handled. Run `/pay` again if you still want to send it.",
                ),
            )
            .await?;
        }
        TakePending::Found(pending) if parts[1] == "cancel" => {
            let _ = pending;
            finish(
                ctx,
                press,
                embeds::info("Payment cancelled", "Nothing was sent."),
            )
            .await?;
        }
        TakePending::Found(pending) => {
            // Saldo revalidado AQUI, na transferencia: entre o /pay e o
            // clique a pessoa pode ter gastado o dinheiro em outro canal.
            let result = transfer(
                &data.economy,
                pending.payer_id,
                pending.target_id,
                pending.amount,
            )
            .await?;

            if !result.done {
                finish(
                    ctx,
                    press,
                    embeds::error(
                        "Payment failed",
                        result
                            .refusal
                            .unwrap_or_else(|| "The payment didn't go through.".to_owned()),
                    ),
                )
                .await?;
                return Ok(());
            }

            let embed = serenity::CreateEmbed::new()
                .title("💸 Payment sent")
                .description(format!(
                    "<@{}> sent **{}** to <@{}>.\n-# Their balance is now {}.",
                    pending.payer_id,
                    store::economy::format(pending.amount),
                    pending.target_id,
                    store::economy::format(result.payer_balance)
                ))
                .colour(embeds::colors::SPRING_GREEN);
            finish(ctx, press, embed).await?;

            tracing::info!(
                "[economia] pay {} -> {}: {}",
                pending.payer_id,
                pending.target_id,
                pending.amount
            );
        }
    }

    Ok(())
}

/// Troca a mensagem do pedido pelo resultado, sem os botoes.
async fn finish(
    ctx: &serenity::Context,
    press: &serenity::ComponentInteraction,
    embed: serenity::CreateEmbed,
) -> Result<(), Error> {
    press
        .create_response(ctx, serenity::CreateInteractionResponse::Acknowledge)
        .await?;
    press
        .edit_response(
            ctx,
            serenity::EditInteractionResponse::new()
                .embed(embed)
                .components(Vec::new())
                .allowed_mentions(serenity::CreateAllowedMentions::new()),
        )
        .await?;
    Ok(())
}

async fn ephemeral_press(
    ctx: &serenity::Context,
    press: &serenity::ComponentInteraction,
    embed: serenity::CreateEmbed,
) -> Result<(), Error> {
    press
        .create_response(
            ctx,
            serenity::CreateInteractionResponse::Message(
                serenity::CreateInteractionResponseMessage::new()
                    .embed(embed)
                    .ephemeral(true),
            ),
        )
        .await?;
    Ok(())
}

pub fn commands() -> Vec<poise::Command<Data, Error>> {
    vec![
        scope::guild_only(daily()),
        scope::guild_only(work_command()),
        scope::guild_only(pay()),
        scope::guild_only(balance()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, m: u32, d: u32, hh: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, hh, 0, 0).unwrap()
    }

    #[tokio::test]
    async fn daily_da_sequencia_e_nao_deixa_reclamar_duas_vezes() {
        let dir = crate::store::test_support::scratch_dir("eco-daily");
        let store = EconomyStore::open(dir.join("economy.json")).await.unwrap();

        let day1 = at(2026, 1, 1, 10);
        let r1 = claim_daily(&store, 1, day1).await.unwrap();
        assert!(r1.claimed);
        assert_eq!(r1.reward, DAILY_BASE);
        assert_eq!(r1.streak, 1);

        // No mesmo dia, nao libera de novo.
        let r1b = claim_daily(&store, 1, at(2026, 1, 1, 23)).await.unwrap();
        assert!(!r1b.claimed);

        // No dia seguinte, a sequencia sobe e o premio cresce.
        let day2 = at(2026, 1, 2, 9);
        let r2 = claim_daily(&store, 1, day2).await.unwrap();
        assert!(r2.claimed);
        assert_eq!(r2.streak, 2);
        assert_eq!(r2.reward, DAILY_BASE + DAILY_STREAK_BONUS);

        // Pulou um dia: a sequencia volta a 1.
        let day4 = at(2026, 1, 4, 9);
        let r4 = claim_daily(&store, 1, day4).await.unwrap();
        assert_eq!(r4.streak, 1);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn work_respeita_o_cooldown_de_uma_hora() {
        let dir = crate::store::test_support::scratch_dir("eco-work");
        let store = EconomyStore::open(dir.join("economy.json")).await.unwrap();

        let t0 = at(2026, 1, 1, 10);
        let r1 = work(&store, 1, t0).await.unwrap();
        assert!(r1.worked);
        assert!((WORK_MIN..=WORK_MAX).contains(&r1.reward));

        let r2 = work(&store, 1, t0 + chrono::Duration::minutes(30))
            .await
            .unwrap();
        assert!(!r2.worked);

        let r3 = work(&store, 1, t0 + chrono::Duration::hours(1))
            .await
            .unwrap();
        assert!(r3.worked);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn transfer_debita_e_credita_juntos_e_recusa_sem_saldo() {
        let dir = crate::store::test_support::scratch_dir("eco-transfer");
        let store = EconomyStore::open(dir.join("economy.json")).await.unwrap();

        store.award(1, 100).await.unwrap();
        let ok = transfer(&store, 1, 2, 40).await.unwrap();
        assert!(ok.done);
        assert_eq!(ok.payer_balance, 60);
        assert_eq!(store.for_user(2).unwrap().balance, 40);

        let short = transfer(&store, 1, 2, 1000).await.unwrap();
        assert!(!short.done);

        let self_pay = transfer(&store, 1, 1, 10).await.unwrap();
        assert!(!self_pay.done);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
