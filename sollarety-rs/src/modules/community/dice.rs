//! Le e rola dados no formato "NdM" ("2d6", "d20") - DiceRoller.cs.
//!
//! Os tetos abaixo nao sao decoracao: sem eles, "999999d999999" faz o bot
//! alocar um vetor gigante e montar uma mensagem que o Discord recusaria de
//! qualquer forma. Um comando de diversao nao pode ser um jeito de derrubar
//! o processo.

use rand::Rng;

pub const MAX_DICE: i32 = 100;
pub const MAX_SIDES: i32 = 1000;

pub struct DiceResult {
    pub count: i32,
    pub sides: i32,
    pub rolls: Vec<i32>,
    pub total: i64,
}

pub fn try_roll(input: &str) -> Result<DiceResult, String> {
    let text = input.trim().to_lowercase().replace(' ', "");
    if text.is_empty() {
        return Err("Enter the dice to roll, for example `2d6` or `d20`.".to_owned());
    }

    let Some(separator) = text.find('d') else {
        return Err("Invalid format. Use `NdM`, for example `2d6`.".to_owned());
    };

    let count_text = &text[..separator];
    let sides_text = &text[separator + 1..];

    // "d20" sem numero na frente quer dizer um dado so.
    let count: i32 = if count_text.is_empty() {
        1
    } else {
        match count_text.parse() {
            Ok(n) => n,
            Err(_) => return Err("Invalid number of dice.".to_owned()),
        }
    };

    let sides: i32 = match sides_text.parse() {
        Ok(n) => n,
        Err(_) => return Err("Invalid number of sides.".to_owned()),
    };

    if !(1..=MAX_DICE).contains(&count) {
        return Err(format!(
            "The number of dice must be between 1 and {MAX_DICE}."
        ));
    }
    if !(2..=MAX_SIDES).contains(&sides) {
        return Err(format!(
            "The number of sides must be between 2 and {MAX_SIDES}."
        ));
    }

    let mut rng = rand::rng();
    let rolls: Vec<i32> = (0..count).map(|_| rng.random_range(1..=sides)).collect();
    let total: i64 = rolls.iter().map(|&r| i64::from(r)).sum();

    Ok(DiceResult {
        count,
        sides,
        rolls,
        total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aceita_formatos_validos() {
        let r = try_roll("2d6").unwrap();
        assert_eq!(r.count, 2);
        assert_eq!(r.sides, 6);
        assert_eq!(r.rolls.len(), 2);
        assert!(r.rolls.iter().all(|&v| (1..=6).contains(&v)));
        assert_eq!(r.total, r.rolls.iter().map(|&v| i64::from(v)).sum::<i64>());

        let one = try_roll("d20").unwrap();
        assert_eq!(one.count, 1);
        assert_eq!(one.sides, 20);

        // Espaco e caixa nao importam.
        assert_eq!(try_roll(" 1D6 ").unwrap().sides, 6);
    }

    #[test]
    fn rejeita_formato_invalido() {
        assert!(try_roll("").is_err());
        assert!(try_roll("6").is_err());
        assert!(try_roll("2dx").is_err());
        assert!(try_roll("xd6").is_err());
    }

    #[test]
    fn rejeita_fora_dos_tetos() {
        assert!(try_roll("0d6").is_err());
        assert!(try_roll("101d6").is_err());
        assert!(try_roll("1d1").is_err());
        assert!(try_roll("1d1001").is_err());
    }
}
