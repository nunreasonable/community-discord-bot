//! Cuida do ESCOPO dos comandos agora que o bot deixou de viver num servidor
//! so (Services/GuildCommandSweeper.cs).
//!
//! O registro global e o modo normal: o Discord anexa os comandos a qualquer
//! servidor onde a aplicacao for instalada com o escopo
//! `applications.commands`. Comando de guild e comando global sao dois
//! registros separados do lado do Discord, entao um servidor que esteve em
//! `guildIds` continua com a copia de guild de pe depois da troca. E ela nao
//! e uma segunda copia que funciona: o despacho casa a interacao pelo id do
//! comando, e o id daquela copia foi criado por um processo anterior. Quem
//! escolhe a errada no menu recebe "a aplicacao nao respondeu".
//!
//! Por isso a varredura nao e arrumacao - e o que faz metade do menu daquele
//! servidor voltar a funcionar.
//!
//! O modo vem de `data.commands_global`, fixado na subida. O modo de errar
//! aqui e varrer em modo de desenvolvimento, apagando os comandos de guild
//! que acabaram de ser registrados; por isso tudo depende dele ser true.

use std::{
    collections::HashSet,
    sync::{LazyLock, Mutex},
};

use poise::serenity_prelude as serenity;
use tracing::{info, warn};

use crate::Data;

/// Servidores ja varridos NESTE processo.
///
/// Em memoria, e nao no guild-settings.json: uma marca persistida escrita
/// por uma subida de desenvolvimento num diretorio de dados diferente
/// ficaria inerte exatamente na travessia para a qual ela existe. O custo de
/// nao persistir e uma consulta barata por servidor por processo.
///
/// Por servidor, e nao um booleano so, porque um servidor pode chegar DEPOIS:
/// o GuildCreate pega quem entra com o bot no ar, mas quem foi convidado com
/// o bot fora chega no READY ja no cache. A varredura de subida e a unica
/// coisa que pega esse caso.
static SWEPT: LazyLock<Mutex<HashSet<u64>>> = LazyLock::new(Mutex::default);

fn swept() -> std::sync::MutexGuard<'static, HashSet<u64>> {
    SWEPT.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(PartialEq, Eq)]
enum SweepResult {
    Clean,
    Removed,
    Failed,
}

/// CacheReady. Devolve NA HORA e faz o trabalho em segundo plano: a
/// varredura e uma consulta REST por servidor, e segurar o despacho dos
/// eventos por ela atrasaria os outros modulos.
///
/// `guilds` e o conjunto que ACABOU de chegar: o cache vivo pode ganhar e
/// perder servidor no meio da varredura.
pub fn on_cache_ready(ctx: &serenity::Context, data: &Data, guilds: Vec<serenity::GuildId>) {
    let ctx = ctx.clone();
    let global = data.commands_global;
    tokio::spawn(async move {
        if !global {
            report_global_leftovers(&ctx).await;
            return;
        }

        let mut removed = 0;
        for guild_id in guilds {
            if sweep_one(&ctx, global, guild_id).await == SweepResult::Removed {
                removed += 1;
            }
        }
        if removed > 0 {
            info!("[comandos] varredura concluida em {removed} servidor(es).");
        }
    });
}

/// Entrada em servidor novo. E aqui que "todo servidor novo que ele entrar"
/// fica VISIVEL: sem esta linha, entrar num servidor e nao funcionar nele
/// produzem o mesmo silencio.
pub fn on_guild_joined(ctx: &serenity::Context, data: &Data, guild: &serenity::Guild) {
    let global = data.commands_global;
    let note = if global {
        "Se o convite trouxe o escopo applications.commands, os comandos globais ja valem aqui, e quem tem Gerenciar Servidor liga o resto com /config."
    } else {
        "AVISO: esta subida registrou comandos POR SERVIDOR (guildIds preenchido), entao aqui nao ha comando nenhum - nem /config."
    };
    let line = format!(
        "[servidor] entrei em \"{}\" ({}), {} membro(s). {note}",
        guild.name, guild.id, guild.member_count
    );
    if global {
        info!("{line}");
    } else {
        warn!("{line}");
    }

    if !global {
        return;
    }

    // O bot pode ja ter estado aqui antes, na epoca do registro por
    // servidor, e ter deixado a copia de guild para tras. Uma consulta so, e
    // so quando entra - nunca no caminho de uma mensagem.
    let ctx = ctx.clone();
    let guild_id = guild.id;
    tokio::spawn(async move {
        sweep_one(&ctx, global, guild_id).await;
    });
}

async fn sweep_one(
    ctx: &serenity::Context,
    global: bool,
    guild_id: serenity::GuildId,
) -> SweepResult {
    // Uma varredura por servidor por processo: o CacheReady re-dispara a
    // cada READY, e sem isto cada queda de socket custaria uma consulta por
    // servidor.
    if !swept().insert(guild_id.get()) {
        return SweepResult::Clean;
    }

    // Isto APAGA todos os comandos de guild do servidor. So e seguro porque,
    // em modo global, o registro da subida nao cria nenhum comando de guild
    // - logo tudo o que estiver la e sobra. Se alguem um dia misturar os
    // dois registros, esta linha passa a apagar o que acabou de ser criado.
    if !global {
        return SweepResult::Clean;
    }

    let result: Result<Vec<serenity::Command>, serenity::Error> = async {
        // Consulta antes de apagar: o DELETE e barato, mas um por servidor a
        // cada processo numa instalacao ja limpa gastaria rate limit a toa e
        // sujaria o log com uma linha que nao aconteceu.
        let existing = ctx.http.get_guild_commands(guild_id).await?;
        if existing.is_empty() {
            return Ok(existing);
        }
        // PUT com a lista vazia: a sobrescrita em lote, que apaga todos.
        ctx.http
            .create_guild_commands(guild_id, &Vec::<serenity::CreateCommand>::new())
            .await?;
        Ok(existing)
    }
    .await;

    match result {
        Ok(existing) if existing.is_empty() => SweepResult::Clean,
        Ok(existing) => {
            // Nomeia o que foi apagado: e uma remocao irreversivel e de uma
            // vez so, e um numero solto nao deixa ninguem conferir depois.
            let names = existing
                .iter()
                .map(|c| format!("/{}", c.name))
                .collect::<Vec<_>>()
                .join(" ");
            info!(
                "[comandos] removi {} comando(s) de servidor que sobravam em {}; os globais assumem no lugar: {names}",
                existing.len(),
                name_of(ctx, guild_id)
            );
            SweepResult::Removed
        }
        // Sem o escopo applications.commands neste servidor nao existe
        // comando de guild para apagar. Nao e falha: e "nao ha nada aqui"
        // dito de outro jeito. Tratar como falha deixaria este servidor
        // pedindo varredura para sempre, com um AVISO por subida.
        Err(cause) if is_unauthorized(&cause) => SweepResult::Clean,
        Err(cause) => {
            // Solta a marca: a varredura so vale como feita quando terminou.
            // O servidor que falhou tenta de novo no proximo READY.
            swept().remove(&guild_id.get());
            warn!(
                "[comandos] AVISO: nao consegui varrer {}: {cause}",
                name_of(ctx, guild_id)
            );
            SweepResult::Failed
        }
    }
}

/// O UnauthorizedException do DisCatSharp: 401 ou 403.
fn is_unauthorized(error: &serenity::Error) -> bool {
    matches!(
        error,
        serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(r))
            if r.status_code == serenity::StatusCode::UNAUTHORIZED
                || r.status_code == serenity::StatusCode::FORBIDDEN
    )
}

fn name_of(ctx: &serenity::Context, guild_id: serenity::GuildId) -> String {
    match ctx.cache.guild(guild_id) {
        Some(guild) => format!("\"{}\" ({guild_id})", guild.name),
        None => guild_id.to_string(),
    }
}

/// O risco espelhado: subir em modo de desenvolvimento com os comandos
/// globais ainda de pe tambem duplica tudo nos servidores da lista.
///
/// Aqui e AVISO, nao remocao - e nao deve virar remocao nem atras de uma
/// flag. Ha um token so e uma aplicacao so: um atalho daqui para "todo
/// servidor de producao fica sem comando" nao pode existir no binario.
async fn report_global_leftovers(ctx: &serenity::Context) {
    match ctx.http.get_global_commands().await {
        Ok(globals) if globals.is_empty() => {}
        Ok(globals) => warn!(
            "[comandos] AVISO: {} comando(s) GLOBAIS continuam registrados, e esta subida registrou por servidor. Nos servidores de guildIds cada comando aparece DUAS vezes, e a copia global responde \"a aplicacao nao respondeu\" - este processo so conhece os ids dos de guild. Nao apago os globais daqui: isso tiraria os comandos de todos os outros servidores. Para voltar ao normal, esvazie guildIds e suba uma vez. Para nao viver com isso, use uma aplicacao e um token separados no desenvolvimento.",
            globals.len()
        ),
        Err(cause) => warn!("[comandos] nao consegui conferir os comandos globais: {cause}"),
    }
}
