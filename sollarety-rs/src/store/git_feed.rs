//! Estado do feed de commits (o `State` privado do GitFeed.cs).
//! data/git-feed.json: repositorio, branch e o ultimo commit postado.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::json_file;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GitFeedState {
    pub repo_path: Option<String>,
    pub branch: Option<String>,
    pub last_commit: Option<String>,
}

/// Estado ilegivel vale como primeira volta (None): posta o commit mais
/// recente e segue dali. Pior caso, um commit repetido no canal.
pub fn load(path: &Path) -> Option<GitFeedState> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!(
                "[gitfeed] aviso: {} ilegivel ({e}); recomecando do commit atual.",
                path.display()
            );
            return None;
        }
    };
    // O DeserializeObject do C# devolve null para arquivo vazio: primeira
    // volta, sem aviso.
    if text.trim().is_empty() {
        return None;
    }
    match serde_json::from_str::<Option<GitFeedState>>(&text) {
        Ok(state) => state,
        Err(e) => {
            tracing::warn!(
                "[gitfeed] aviso: {} ilegivel ({e}); recomecando do commit atual.",
                path.display()
            );
            None
        }
    }
}

/// Grava por .tmp + rename: um crash no meio nao deixa o arquivo pela metade.
pub fn save(path: &Path, state: &GitFeedState) -> Result<(), crate::Error> {
    json_file::write_text_atomic(path, &json_file::to_text(state)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::{copy_real, scratch_dir};

    #[test]
    fn ida_e_volta_do_arquivo_real() {
        let dir = scratch_dir("git-feed");
        let Some(path) = copy_real(&dir, "git-feed.json") else {
            return;
        };
        let original = std::fs::read_to_string(&path).unwrap();
        let state = load(&path).unwrap();
        save(&path, &state).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ausente_vazio_ou_lixo_e_primeira_volta() {
        let dir = scratch_dir("git-feed-bad");
        let path = dir.join("git-feed.json");
        assert!(load(&path).is_none());
        std::fs::write(&path, "").unwrap();
        assert!(load(&path).is_none());
        std::fs::write(&path, "{").unwrap();
        assert!(load(&path).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
