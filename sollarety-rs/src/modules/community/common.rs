//! Pecinhas usadas por mais de um arquivo deste modulo: nome de exibicao
//! (Levels.NameOf) e o arredondamento "banker's rounding" que o `Math.Round`
//! do C# usa por padrao.

use poise::serenity_prelude as serenity;

/// Levels.NameOf: apelido no servidor quando a pessoa esta nele (olha o
/// cache antes de qualquer requisicao - `hierarchy::try_get_member` ja faz
/// isso); senao o nome global da conta.
pub async fn display_name(
    http: impl serenity::CacheHttp,
    guild_id: Option<serenity::GuildId>,
    user: &serenity::User,
) -> String {
    if let Some(guild_id) = guild_id
        && let Ok(Some(member)) = crate::hierarchy::try_get_member(http, guild_id, user.id).await
    {
        return member.display_name().to_owned();
    }
    global_name(user)
}

/// `string.IsNullOrWhiteSpace(user.GlobalName) ? user.Username : user.GlobalName`.
pub fn global_name(user: &serenity::User) -> String {
    match &user.global_name {
        Some(name) if !name.trim().is_empty() => name.clone(),
        _ => user.name.clone(),
    }
}

/// `Math.Round(double)` do C# arredonda para o PAR mais proximo num empate
/// exato em .5 ("banker's rounding"), diferente do `f64::round` do Rust, que
/// arredonda para longe do zero. So importa nos empates exatos - raro nas
/// barras de progresso que usam isto, mas e o que o C# faz.
pub fn round_half_even(value: f64) -> f64 {
    let floor = value.floor();
    let diff = value - floor;
    if diff < 0.5 {
        floor
    } else if diff > 0.5 {
        floor + 1.0
    } else if (floor as i64) % 2 == 0 {
        floor
    } else {
        floor + 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arredonda_para_o_par_no_empate() {
        assert_eq!(round_half_even(0.5), 0.0);
        assert_eq!(round_half_even(1.5), 2.0);
        assert_eq!(round_half_even(2.5), 2.0);
        assert_eq!(round_half_even(3.5), 4.0);
        assert_eq!(round_half_even(2.4), 2.0);
        assert_eq!(round_half_even(2.6), 3.0);
        assert_eq!(round_half_even(-0.5), 0.0);
    }
}
