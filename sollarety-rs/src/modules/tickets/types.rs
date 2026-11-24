//! Os tipos de ticket, num lugar so (TicketTypes.cs): o que vira botao no
//! painel, opcao do /ticket e prefixo de canal.

use std::borrow::Cow;

use poise::CommandParameterChoice;

/// Um tipo de ticket: o que vira botao no painel e prefixo de canal.
#[derive(Debug, PartialEq, Eq)]
pub struct TicketType {
    pub key: &'static str,
    pub label: &'static str,
    pub emoji: &'static str,
    pub channel_prefix: &'static str,
}

/// Os tipos disponiveis. O painel quebra os botoes em linhas de cinco (o
/// teto de uma action row), entao um quinto tipo aqui nao derruba nada.
pub const ALL: &[TicketType] = &[
    TicketType {
        key: "duvida",
        label: "Question",
        emoji: "❓",
        channel_prefix: "question",
    },
    TicketType {
        key: "denuncia",
        label: "Report",
        emoji: "🚨",
        channel_prefix: "report",
    },
    TicketType {
        key: "parceria",
        label: "Partnership",
        emoji: "🤝",
        channel_prefix: "partnership",
    },
    TicketType {
        key: "outro",
        label: "Other",
        emoji: "📩",
        channel_prefix: "other",
    },
];

/// O tipo, ou None se a chave nao existe mais.
///
/// Nao falha de proposito: um botao de painel publicado ha meses carrega a
/// chave no custom id, e alguem pode ter removido aquele tipo desta lista
/// nesse meio-tempo. Quem chama trata o None com uma mensagem, em vez de o
/// clique virar "interacao falhou".
pub fn find(key: &str) -> Option<&'static TicketType> {
    ALL.iter().find(|t| t.key.eq_ignore_ascii_case(key))
}

/// Rotulo legivel para um tipo que talvez nao exista mais.
pub fn describe(key: &str) -> String {
    match find(key) {
        Some(t) => t.label.to_owned(),
        None if key.is_empty() => "(unknown)".to_owned(),
        None => key.to_owned(),
    }
}

/// A opcao "type" do /ticket, gerada a partir de `ALL`.
///
/// Existe pela mesma razao do TicketTypeChoiceProvider do C#: com os tipos
/// escritos a mao em dois lugares (um enum com `#[derive(ChoiceParameter)]`
/// e esta lista), acrescentar um tipo aqui fazia o painel ganhar o botao novo
/// e o /ticket NAO ganhar a opcao, sem erro nenhum.
#[derive(Clone, Copy, Debug)]
pub struct TypeChoice(pub &'static TicketType);

impl poise::ChoiceParameter for TypeChoice {
    fn list() -> Cow<'static, [CommandParameterChoice]> {
        ALL.iter()
            .map(|t| CommandParameterChoice {
                name: t.label.into(),
                localizations: Default::default(),
                __non_exhaustive: (),
            })
            .collect::<Vec<_>>()
            .into()
    }

    fn from_index(index: usize) -> Option<Self> {
        ALL.get(index).map(Self)
    }

    fn from_name(name: &str) -> Option<Self> {
        ALL.iter()
            .find(|t| t.label.eq_ignore_ascii_case(name) || t.key.eq_ignore_ascii_case(name))
            .map(Self)
    }

    fn name(&self) -> &'static str {
        self.0.label
    }

    fn localized_name(&self, _locale: &str) -> Option<&'static str> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poise::ChoiceParameter;

    #[test]
    fn acha_sem_diferenciar_caixa_e_descreve_o_que_sumiu() {
        assert_eq!(find("DENUNCIA").unwrap().label, "Report");
        assert!(find("sumiu").is_none());
        assert_eq!(describe("parceria"), "Partnership");
        assert_eq!(describe("sumiu"), "sumiu");
        assert_eq!(describe(""), "(unknown)");
    }

    #[test]
    fn escolhas_saem_da_mesma_lista() {
        let list = TypeChoice::list();
        assert_eq!(list.len(), ALL.len());
        assert_eq!(list[0].name, "Question");
        assert_eq!(TypeChoice::from_index(1).unwrap().0.key, "denuncia");
        assert!(TypeChoice::from_index(ALL.len()).is_none());
        assert_eq!(TypeChoice::from_name("Other").unwrap().0.key, "outro");
    }
}
