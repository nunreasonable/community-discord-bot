//! O json_schema que o /quiz manda para o llama.cpp, e a validacao da
//! resposta (QuizSchema.cs). Sem Discord nem rede aqui de proposito: e so
//! JSON puro, o que deixa testar sem precisar simular um servidor.

use serde_json::{Value, json};

use crate::embeds;

/// Uma pergunta ja validada: 4 opcoes distintas e uma resposta certa entre elas.
#[derive(Clone, Debug, PartialEq)]
pub struct QuizQuestion {
    pub question: String,
    pub options: [String; 4],
    pub answer: usize,
}

pub fn build(count: usize) -> Value {
    json!({
        "type": "object",
        "properties": {
            "questions": {
                "type": "array",
                "minItems": count,
                "maxItems": count,
                "items": {
                    "type": "object",
                    "properties": {
                        "question": { "type": "string" },
                        "options": {
                            "type": "array",
                            "minItems": 4,
                            "maxItems": 4,
                            "items": { "type": "string" }
                        },
                        "answer": { "type": "integer", "minimum": 0, "maximum": 3 }
                    },
                    "required": ["question", "options", "answer"]
                }
            }
        },
        "required": ["questions"]
    })
}

/// So aceita a resposta do modelo se bater TUDO: a contagem exata de
/// perguntas, 4 opcoes nao vazias e distintas (sem diferenca de
/// maiuscula/minuscula) por pergunta, e a resposta dentro do intervalo. Um
/// JSON "quase certo" (3 opcoes, repetida, indice fora da faixa) vira um
/// quiz sem clique certo - melhor nao postar nada.
pub fn parse_and_validate(raw: Option<&str>, expected_count: usize) -> Option<Vec<QuizQuestion>> {
    let raw = raw?;
    if raw.trim().is_empty() {
        return None;
    }

    let root: Value = serde_json::from_str(raw).ok()?;
    let items = root.get("questions")?.as_array()?;
    if items.len() != expected_count {
        return None;
    }

    let mut result = Vec::with_capacity(expected_count);
    for item in items {
        let obj = item.as_object()?;

        let question = obj.get("question")?.as_str()?.trim().to_owned();
        if question.is_empty() || embeds::utf16_len(&question) > 300 {
            return None;
        }

        let opts = obj.get("options")?.as_array()?;
        if opts.len() != 4 {
            return None;
        }
        let options: Vec<String> = opts
            .iter()
            .map(|o| o.as_str().unwrap_or_default().trim().to_owned())
            .collect();
        if options
            .iter()
            .any(|o| o.is_empty() || embeds::utf16_len(o) > 150)
        {
            return None;
        }

        let mut distinct: Vec<String> = options.iter().map(|o| o.to_lowercase()).collect();
        distinct.sort_unstable();
        distinct.dedup();
        if distinct.len() != 4 {
            return None;
        }

        // Precisa ser um inteiro JSON de verdade (1.0 ou "1" nao valem), como
        // o `Type: JTokenType.Integer` do C#.
        let answer = obj.get("answer")?.as_i64()?;
        if !(0..=3).contains(&answer) {
            return None;
        }

        let Ok(options): Result<[String; 4], _> = options.try_into() else {
            return None;
        };
        result.push(QuizQuestion {
            question,
            options,
            answer: answer as usize,
        });
    }

    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_json(count: usize) -> String {
        let questions: Vec<_> = (0..count)
            .map(|i| {
                json!({
                    "question": format!("Pergunta {i}?"),
                    "options": ["A", "B", "C", "D"],
                    "answer": 1
                })
            })
            .collect();
        json!({ "questions": questions }).to_string()
    }

    #[test]
    fn aceita_quiz_valido() {
        let parsed = parse_and_validate(Some(&valid_json(3)), 3).unwrap();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].options, ["A", "B", "C", "D"]);
        assert_eq!(parsed[0].answer, 1);
    }

    #[test]
    fn recusa_entrada_vazia_ou_invalida() {
        assert_eq!(parse_and_validate(None, 3), None);
        assert_eq!(parse_and_validate(Some(""), 3), None);
        assert_eq!(parse_and_validate(Some("not json"), 3), None);
        assert_eq!(parse_and_validate(Some("[]"), 3), None);
    }

    #[test]
    fn recusa_contagem_errada_de_perguntas() {
        assert_eq!(parse_and_validate(Some(&valid_json(2)), 3), None);
        assert_eq!(parse_and_validate(Some(&valid_json(4)), 3), None);
    }

    #[test]
    fn recusa_menos_de_quatro_opcoes() {
        let raw = json!({ "questions": [ { "question": "q?", "options": ["A", "B", "C"], "answer": 0 } ] })
            .to_string();
        assert_eq!(parse_and_validate(Some(&raw), 1), None);
    }

    #[test]
    fn recusa_opcoes_repetidas_ignorando_caixa() {
        let raw = json!({ "questions": [ { "question": "q?", "options": ["A", "a", "B", "C"], "answer": 0 } ] })
            .to_string();
        assert_eq!(parse_and_validate(Some(&raw), 1), None);
    }

    #[test]
    fn recusa_opcao_vazia() {
        let raw = json!({ "questions": [ { "question": "q?", "options": ["", "B", "C", "D"], "answer": 0 } ] })
            .to_string();
        assert_eq!(parse_and_validate(Some(&raw), 1), None);
    }

    #[test]
    fn recusa_resposta_fora_do_intervalo() {
        let raw = json!({ "questions": [ { "question": "q?", "options": ["A", "B", "C", "D"], "answer": 4 } ] })
            .to_string();
        assert_eq!(parse_and_validate(Some(&raw), 1), None);
        let raw = json!({ "questions": [ { "question": "q?", "options": ["A", "B", "C", "D"], "answer": -1 } ] })
            .to_string();
        assert_eq!(parse_and_validate(Some(&raw), 1), None);
    }

    #[test]
    fn recusa_resposta_nao_inteira() {
        let raw = json!({ "questions": [ { "question": "q?", "options": ["A", "B", "C", "D"], "answer": 1.5 } ] })
            .to_string();
        assert_eq!(parse_and_validate(Some(&raw), 1), None);
        let raw = json!({ "questions": [ { "question": "q?", "options": ["A", "B", "C", "D"], "answer": "1" } ] })
            .to_string();
        assert_eq!(parse_and_validate(Some(&raw), 1), None);
    }

    #[test]
    fn recusa_pergunta_vazia_ou_longa_demais() {
        let raw = json!({ "questions": [ { "question": "  ", "options": ["A", "B", "C", "D"], "answer": 0 } ] })
            .to_string();
        assert_eq!(parse_and_validate(Some(&raw), 1), None);
        let raw = json!({ "questions": [ { "question": "x".repeat(301), "options": ["A", "B", "C", "D"], "answer": 0 } ] })
            .to_string();
        assert_eq!(parse_and_validate(Some(&raw), 1), None);
    }
}
