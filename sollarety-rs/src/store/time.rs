//! Datas no formato que o Newtonsoft da a um DateTimeOffset:
//! `2026-10-06T18:42:54.0164367+00:00`. Sete casas (ticks de 100 ns), zeros
//! do fim cortados, e o ponto some quando a fracao e zero.
//!
//! Tudo e guardado em UTC. O C# so grava `DateTimeOffset.UtcNow`, entao o
//! deslocamento nos arquivos e sempre +00:00; um deslocamento diferente
//! escrito a mao e lido certo e volta como o mesmo instante em UTC.

use chrono::{DateTime, NaiveDate, SubsecRound, Utc};
use serde::{Deserialize, Deserializer, Serializer};

pub type Time = DateTime<Utc>;

/// `DateTime<Utc>::now()` cortado em ticks, para o que fica em memoria ser
/// igual ao que volta do disco.
pub fn now() -> Time {
    Utc::now().trunc_subsecs(7)
}

/// O `DateTimeOffset.MinValue`, que o C# deixa num campo de data ausente.
pub fn min_value() -> Time {
    NaiveDate::from_ymd_opt(1, 1, 1)
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|d| d.and_utc())
        .unwrap_or(DateTime::<Utc>::MIN_UTC)
}

pub fn format(time: &Time) -> String {
    let mut text = time.format("%Y-%m-%dT%H:%M:%S").to_string();
    let ticks = time.timestamp_subsec_nanos() / 100;
    if ticks > 0 {
        let fraction = format!("{ticks:07}");
        text.push('.');
        text.push_str(fraction.trim_end_matches('0'));
    }
    text.push_str("+00:00");
    text
}

pub fn parse(text: &str) -> Result<Time, chrono::ParseError> {
    DateTime::parse_from_rfc3339(text).map(|t| t.with_timezone(&Utc))
}

pub fn serialize<S: Serializer>(time: &Time, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&format(time))
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Time, D::Error> {
    // null num campo nao anulavel: o Newtonsoft recusaria; aqui vira o
    // MinValue, que e o que um campo ausente daria.
    match Option::<String>::deserialize(d)? {
        Some(text) => parse(&text).map_err(serde::de::Error::custom),
        None => Ok(min_value()),
    }
}

/// O mesmo, para `DateTimeOffset?`.
pub mod option {
    use super::*;

    pub fn serialize<S: Serializer>(time: &Option<Time>, s: S) -> Result<S::Ok, S::Error> {
        match time {
            Some(time) => s.serialize_str(&format(time)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Time>, D::Error> {
        Option::<String>::deserialize(d)?
            .map(|text| parse(&text).map_err(serde::de::Error::custom))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formata_como_o_newtonsoft() {
        for text in [
            "2026-10-06T18:42:54.0164367+00:00",
            "2026-10-06T18:42:54.5+00:00",
            "2026-10-06T18:42:54+00:00",
            "0001-01-01T00:00:00+00:00",
        ] {
            assert_eq!(format(&parse(text).unwrap()), text);
        }
        assert_eq!(format(&min_value()), "0001-01-01T00:00:00+00:00");
    }

    #[test]
    fn le_outro_deslocamento_como_o_mesmo_instante() {
        let t = parse("2026-10-06T15:42:54-03:00").unwrap();
        assert_eq!(format(&t), "2026-10-06T18:42:54+00:00");
    }

    #[test]
    fn now_cabe_em_ticks() {
        let t = now();
        assert_eq!(parse(&format(&t)).unwrap(), t);
    }
}
