//! Roda um programa externo (yt-dlp, ffmpeg, git) ate o fim, com prazo
//! (ProcessRunner.cs).
//!
//! Argumentos sempre como lista: nada de linha de comando montada em string,
//! que viraria injecao de opcao pelo primeiro titulo de video ou mensagem de
//! commit com aspas.
//!
//! Cada filho nasce no PROPRIO grupo de processos, e estourar o prazo (ou
//! largar o future, que e como se cancela aqui) mata o grupo inteiro: o
//! yt-dlp abre o node e as vezes o ffmpeg por baixo, e matar so o pai deixaria
//! os filhos segurando arquivo.

use std::{ffi::OsStr, path::Path, process::Stdio, time::Duration};

use tokio::process::Command;

use crate::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessResult {
    /// -1 quando o processo morreu por sinal.
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Roda e espera. Saida em UTF-8 (com troca do que nao for valido).
pub async fn run<I, S>(
    executable: &str,
    args: I,
    timeout: Duration,
    env: &[(&str, &str)],
) -> Result<ProcessResult, Error>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    for (key, value) in env {
        command.env(key, value);
    }

    let child = command
        .spawn()
        .map_err(|e| format!("nao consegui executar '{executable}': {e}"))?;
    let _group = GroupGuard(child.id());

    // wait_with_output le os dois fluxos junto com a espera: um stderr cheio e
    // nunca lido trava o filho no write.
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(output) => {
            let output = output?;
            Ok(ProcessResult {
                exit_code: output.status.code().unwrap_or(-1),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            })
        }
        Err(_) => {
            let name = Path::new(executable)
                .file_name()
                .map_or(executable.into(), |n| n.to_string_lossy());
            Err(format!("'{name}' passou de {}s", timeout.as_secs()).into())
        }
    }
}

/// Mata o grupo inteiro do processo `pid` (o filho criado com
/// `process_group(0)`, cujo grupo tem o mesmo numero). Para quem gerencia o
/// proprio `Child`, como o ffmpeg que alimenta a voz.
pub fn kill_tree(pid: u32) {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return;
    };
    // SAFETY: kill(2) nao mexe em memoria; um grupo que ja acabou so devolve
    // ESRCH, que nao interessa.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
}

/// Ao sair do `run` (fim normal, prazo ou future largado), mata o que sobrou
/// do grupo. No fim normal o pai ja saiu e isto so pega neto esquecido.
struct GroupGuard(Option<u32>);

impl Drop for GroupGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            kill_tree(pid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn devolve_saida_e_codigo() {
        let result = run(
            "sh",
            ["-c", "printf 'oi\\n'; echo erro >&2; exit 3"],
            Duration::from_secs(5),
            &[],
        )
        .await
        .unwrap();
        assert_eq!(result.exit_code, 3);
        assert_eq!(result.stdout, "oi\n");
        assert_eq!(result.stderr, "erro\n");
    }

    #[tokio::test]
    async fn passa_ambiente_e_estoura_prazo() {
        let result = run(
            "sh",
            ["-c", "echo $X"],
            Duration::from_secs(5),
            &[("X", "y")],
        )
        .await
        .unwrap();
        assert_eq!(result.stdout, "y\n");

        let err = run("sh", ["-c", "sleep 5"], Duration::from_millis(200), &[])
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "'sh' passou de 0s");
    }

    #[tokio::test]
    async fn executavel_que_nao_existe_e_erro() {
        assert!(
            run("/nao/existe", [""; 0], Duration::from_secs(1), &[])
                .await
                .is_err()
        );
    }
}
