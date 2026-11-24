//! Verificacao Roblox: /verify, /unverify, /update, /whois, /verify-panel,
//! os botoes "verify:", os binds de grupo (/bind) e a entrada de membro
//! (cargos e apelido de quem ja e vinculado).
//!
//! - `commands`: a porta de entrada (Verification.cs)
//! - `binds`: /bind add|remove|list (Binds.cs)
//! - `flow`: o /verify, os botoes e o polling do Worker (VerificationFlow.cs)
//! - `service`: cargos e apelido num servidor (VerificationService.cs)
//! - `api`: a API publica do Roblox, com cache de 5 min (RobloxApi.cs)
//! - `worker`: o resultado do OAuth no Worker (VerifyWorkerClient.cs)

mod api;
mod binds;
mod commands;
mod flow;
mod service;
mod worker;

use poise::serenity_prelude as serenity;

use crate::{Data, Error};

/// Os comandos do modulo, na ordem do RegisterAll do C# (Verification e
/// depois Binds).
pub fn commands() -> Vec<poise::Command<Data, Error>> {
    let mut all = commands::commands();
    all.extend(binds::commands());
    all
}

/// So os botoes "verify:" e a entrada de membro sao daqui. O trabalho vai num
/// `tokio::spawn`, para nao segurar os outros modulos.
pub async fn on_event(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    data: &Data,
) -> Result<(), Error> {
    match event {
        serenity::FullEvent::InteractionCreate {
            interaction: serenity::Interaction::Component(component),
        } if component.data.custom_id.starts_with(flow::PREFIX) => {
            flow::on_component(ctx, data, component);
        }
        serenity::FullEvent::GuildMemberAddition { new_member } => {
            service::on_member_added(ctx, data, new_member);
        }
        _ => {}
    }
    Ok(())
}

/// O C# nao esperava nada aqui; os pollings pendentes so param, para nao
/// consultar o Worker com o gateway ja fechado.
pub async fn shutdown(_data: &Data) {
    flow::stop_all();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comandos_e_contextos() {
        let all = commands();
        let names: Vec<&str> = all.iter().map(|c| &*c.name).collect();
        assert_eq!(
            names,
            [
                "verify",
                "unverify",
                "update",
                "whois",
                "verify-panel",
                "bind"
            ]
        );
        // So o /unverify funciona fora de servidor e como app pessoal.
        for command in &all {
            let contexts = command.interaction_context.as_ref().map(Vec::len);
            if command.name == "unverify" {
                assert_eq!(contexts, Some(3));
                assert_eq!(command.install_context.as_ref().map(Vec::len), Some(2));
            } else {
                assert_eq!(contexts, Some(1), "{}", command.name);
            }
        }
        let panel = all.iter().find(|c| c.name == "verify-panel").unwrap();
        assert_eq!(
            panel.default_member_permissions,
            serenity::Permissions::MANAGE_GUILD
        );
    }
}
