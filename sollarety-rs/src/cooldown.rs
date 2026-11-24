//! O `[SlashCommandCooldown(usos, segundos, User)]` do DisCatSharp: N usos
//! por janela, por pessoa e por comando. O cooldown do poise so conhece "um
//! uso por janela", e o bot usa 3 a cada 10s, 2 a cada 30s etc.
//!
//! Uso, como check do comando:
//!
//! ```ignore
//! #[poise::command(slash_command, check = "crate::cooldown::user::<3, 10>")]
//! ```
//!
//! Estourou: o check devolve `CooldownHit` como erro, e o `errors::on_error`
//! responde em efemero com a mesma frase do DisCatSharp.

use std::{
    collections::HashMap,
    fmt,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};

use crate::{Context, Error};

/// O balde estourou; volta a valer em `resets_at`.
#[derive(Debug)]
pub struct CooldownHit {
    pub resets_at: DateTime<Utc>,
}

impl fmt::Display for CooldownHit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cooldown ate {}", self.resets_at.format("%H:%M:%S"))
    }
}

impl std::error::Error for CooldownHit {}

struct Bucket {
    remaining: u32,
    resets_at: Instant,
    resets_wall: DateTime<Utc>,
}

#[derive(Default)]
struct Buckets(HashMap<(String, u64), Bucket>);

impl Buckets {
    /// A janela comeca no primeiro uso, como no DisCatSharp, e nao desliza.
    fn hit(
        &mut self,
        now: Instant,
        command: &str,
        user: u64,
        uses: u32,
        window: Duration,
    ) -> Result<(), CooldownHit> {
        // Faxina barata: so quando o mapa cresce.
        if self.0.len() > 4096 {
            self.0.retain(|_, b| b.resets_at > now);
        }

        let bucket = self
            .0
            .entry((command.to_owned(), user))
            .or_insert_with(|| Bucket {
                remaining: uses,
                resets_at: now + window,
                resets_wall: Utc::now() + window,
            });
        if now >= bucket.resets_at {
            *bucket = Bucket {
                remaining: uses,
                resets_at: now + window,
                resets_wall: Utc::now() + window,
            };
        }
        if bucket.remaining == 0 {
            return Err(CooldownHit {
                resets_at: bucket.resets_wall,
            });
        }
        bucket.remaining -= 1;
        Ok(())
    }
}

static BUCKETS: LazyLock<Mutex<Buckets>> = LazyLock::new(Mutex::default);

/// Conta um uso de `command` por `user`. Para quem precisa do cooldown fora
/// de um check (um botao, por exemplo).
pub fn hit(command: &str, user: u64, uses: u32, window: Duration) -> Result<(), CooldownHit> {
    BUCKETS.lock().unwrap_or_else(|e| e.into_inner()).hit(
        Instant::now(),
        command,
        user,
        uses,
        window,
    )
}

/// Check do poise: `USES` usos a cada `SECS` segundos, por pessoa.
pub async fn user<const USES: u32, const SECS: u64>(ctx: Context<'_>) -> Result<bool, Error> {
    hit(
        &ctx.command().qualified_name,
        ctx.author().id.get(),
        USES,
        Duration::from_secs(SECS),
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prova que o check com turbofish passa pelo macro do poise.
    #[poise::command(slash_command, check = "crate::cooldown::user::<3, 10>")]
    async fn exemplo(ctx: Context<'_>) -> Result<(), Error> {
        ctx.say("ok").await?;
        Ok(())
    }

    #[test]
    fn check_entra_no_comando() {
        let command = crate::scope::anywhere(exemplo());
        assert_eq!(command.checks.len(), 1);
        assert_eq!(command.install_context.as_ref().map(Vec::len), Some(2));
    }

    #[test]
    fn n_usos_por_janela_por_pessoa_e_comando() {
        let mut buckets = Buckets::default();
        let t0 = Instant::now();
        let window = Duration::from_secs(10);

        for _ in 0..3 {
            assert!(buckets.hit(t0, "roll", 1, 3, window).is_ok());
        }
        assert!(
            buckets
                .hit(t0 + Duration::from_secs(9), "roll", 1, 3, window)
                .is_err()
        );
        // Outra pessoa e outro comando tem balde proprio.
        assert!(buckets.hit(t0, "roll", 2, 3, window).is_ok());
        assert!(buckets.hit(t0, "ship", 1, 3, window).is_ok());
        // A janela conta do primeiro uso.
        assert!(buckets.hit(t0 + window, "roll", 1, 3, window).is_ok());
    }
}
