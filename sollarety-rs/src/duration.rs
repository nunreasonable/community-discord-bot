//! Duracao escrita como gente escreve: "10m", "2h30m", "1d", "45s"
//! (DurationParser.cs). Varias unidades na mesma string se somam.

use std::time::Duration;

/// Teto do timeout no Discord. Pedir mais e recusado pela API, entao e
/// melhor recusar aqui com uma mensagem que explica.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(28 * 24 * 3600);

/// Ticks de 100 ns, a unidade do TimeSpan: o estouro acontece no mesmo
/// ponto que no C#.
const TICKS_PER_SECOND: i128 = 10_000_000;

/// A duracao, ou a frase (em ingles, como no bot) que explica o erro.
pub fn parse(input: &str) -> Result<Duration, String> {
    let text = input.trim().to_lowercase();
    if text.is_empty() {
        return Err("Enter a duration, for example `10m`, `2h30m` or `1d`.".into());
    }

    let max_ticks = MAX_TIMEOUT.as_secs() as i128 * TICKS_PER_SECOND;
    let mut total: i128 = 0;
    let mut number = String::new();
    let mut saw_unit = false;

    for ch in text.chars() {
        // ASCII 0-9 apenas: o char::is_numeric aceitaria digitos de outras
        // escritas, que o parse depois recusaria com a mensagem errada.
        if ch.is_ascii_digit() {
            number.push(ch);
            continue;
        }
        if ch == ' ' {
            continue;
        }
        if number.is_empty() {
            return Err(format!(
                "Missing a number before `{ch}`. Use something like `10m` or `2h30m`."
            ));
        }

        let value: i64 = number.parse().map_err(|_| "Number too large.".to_owned())?;

        let unit_seconds: i128 = match ch {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            'd' => 86_400,
            _ => return Err(format!("Unknown unit `{ch}`. Use `s`, `m`, `h` or `d`.")),
        };

        total += unit_seconds * TICKS_PER_SECOND * value as i128;
        if total > i64::MAX as i128 {
            return Err("Duration too large.".into());
        }
        if total > max_ticks {
            return Err(format!(
                "Discord doesn't allow more than {} days.",
                MAX_TIMEOUT.as_secs() / 86_400
            ));
        }

        number.clear();
        saw_unit = true;
    }

    // Numero sem unidade no fim ("10") seria adivinhacao.
    if !number.is_empty() {
        return Err(
            "Missing a unit at the end. Use `s`, `m`, `h` or `d` — for example `10m`.".into(),
        );
    }
    if !saw_unit || total <= 0 {
        return Err("The duration must be greater than zero.".into());
    }

    Ok(Duration::from_secs((total / TICKS_PER_SECOND) as u64))
}

/// A duracao de volta em ingles, para o embed de confirmacao.
pub fn describe(duration: Duration) -> String {
    let secs = duration.as_secs();
    let (days, hours, minutes, seconds) =
        (secs / 86_400, secs / 3600 % 24, secs / 60 % 60, secs % 60);

    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days} day(s)"));
    }
    if hours > 0 {
        parts.push(format!("{hours} hour(s)"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes} minute(s)"));
    }
    if seconds > 0 {
        parts.push(format!("{seconds} second(s)"));
    }

    if parts.is_empty() {
        "0 seconds".into()
    } else {
        parts.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soma_unidades() {
        assert_eq!(parse("10m"), Ok(Duration::from_secs(600)));
        assert_eq!(parse(" 2H 30m "), Ok(Duration::from_secs(9000)));
        assert_eq!(parse("1d1s"), Ok(Duration::from_secs(86_401)));
        assert_eq!(parse("28d"), Ok(MAX_TIMEOUT));
    }

    #[test]
    fn recusa_com_a_mesma_frase_do_csharp() {
        assert_eq!(
            parse("").unwrap_err(),
            "Enter a duration, for example `10m`, `2h30m` or `1d`."
        );
        assert_eq!(
            parse("m").unwrap_err(),
            "Missing a number before `m`. Use something like `10m` or `2h30m`."
        );
        assert_eq!(
            parse("10x").unwrap_err(),
            "Unknown unit `x`. Use `s`, `m`, `h` or `d`."
        );
        assert_eq!(
            parse("10").unwrap_err(),
            "Missing a unit at the end. Use `s`, `m`, `h` or `d` — for example `10m`."
        );
        assert_eq!(
            parse("0m").unwrap_err(),
            "The duration must be greater than zero."
        );
        assert_eq!(
            parse("29d").unwrap_err(),
            "Discord doesn't allow more than 28 days."
        );
        assert_eq!(
            parse("99999999999999999999s").unwrap_err(),
            "Number too large."
        );
        assert_eq!(parse("9999999999999d").unwrap_err(), "Duration too large.");
        // Digito de outra escrita nao e numero.
        assert!(
            parse("٣m")
                .unwrap_err()
                .starts_with("Missing a number before")
        );
    }

    #[test]
    fn descreve() {
        assert_eq!(
            describe(Duration::from_secs(90061)),
            "1 day(s), 1 hour(s), 1 minute(s), 1 second(s)"
        );
        assert_eq!(describe(Duration::from_secs(600)), "10 minute(s)");
        assert_eq!(describe(Duration::ZERO), "0 seconds");
    }
}
