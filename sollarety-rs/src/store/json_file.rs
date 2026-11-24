//! Leitura e escrita atomica de um arquivo JSON de data/ (JsonFile.cs).

use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Serialize, de::DeserializeOwned};

use crate::Error;

/// Arquivo ausente, vazio (ou so espacos) ou com o literal `null` vira o
/// padrao. Arquivo que nao desserializa e ERRO: tratado como vazio, a proxima
/// escrita consolidaria "nada" por cima dos dados, em silencio.
pub async fn read<T: DeserializeOwned + Default>(path: &Path, tag: &str) -> Result<T, Error> {
    let text = match tokio::fs::read_to_string(path).await {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(T::default()),
        Err(e) => {
            tracing::error!("[{tag}] falha ao ler {}: {e}", path.display());
            return Err(e.into());
        }
    };
    parse(&text).map_err(|e| {
        tracing::error!("[{tag}] falha ao ler {}: {e}", path.display());
        e
    })
}

/// A parte pura do `read`, separada para os testes.
pub fn parse<T: DeserializeOwned + Default>(text: &str) -> Result<T, Error> {
    // O File.ReadAllText do C# engole o BOM; o serde_json nao.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.trim().is_empty() {
        return Ok(T::default());
    }
    Ok(serde_json::from_str::<Option<T>>(text)?.unwrap_or_default())
}

/// O texto exato que vai para o disco: o Formatting.Indented do Newtonsoft
/// (dois espacos, `"chave": valor`, sem quebra de linha no fim) e o mesmo do
/// pretty printer do serde_json.
pub fn to_text<T: Serialize>(value: &T) -> Result<String, Error> {
    Ok(serde_json::to_string_pretty(value)?)
}

/// Grava num .tmp, sincroniza no disco e SO ENTAO renomeia por cima. Sem o
/// fsync, uma queda de energia podia levar o rename ao disco antes do
/// conteudo, e o que sobrava era um arquivo vazio - que a leitura trataria
/// como "nenhum dado".
pub async fn write_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), Error> {
    let text = to_text(value)?;
    let path = path.to_path_buf();
    // Bloqueante (fsync), entao fora das threads do runtime.
    tokio::task::spawn_blocking(move || write_text_atomic(&path, &text)).await??;
    Ok(())
}

pub fn write_text_atomic(path: &Path, text: &str) -> io::Result<()> {
    let temp = temp_path(path);
    {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&temp, path)
}

/// Um .tmp que sobrou de um processo morto no meio da escrita. Best-effort:
/// se nao der para apagar, a proxima escrita o sobrescreve.
pub fn delete_leftover_temp(path: &Path, tag: &str) {
    let temp = temp_path(path);
    match std::fs::remove_file(&temp) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!("[{tag}] aviso: nao consegui apagar {}: {e}", temp.display()),
    }
}

/// `warnings.json` -> `warnings.json.tmp`, como o `path + ".tmp"` do C#.
pub fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".tmp");
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
    struct Sample {
        #[serde(default)]
        items: Vec<u64>,
    }

    #[test]
    fn vazio_e_null_viram_o_padrao_e_lixo_e_erro() {
        assert_eq!(parse::<Sample>("").unwrap(), Sample::default());
        assert_eq!(parse::<Sample>("  \n").unwrap(), Sample::default());
        assert_eq!(parse::<Sample>("null").unwrap(), Sample::default());
        assert_eq!(
            parse::<Sample>("\u{feff}{\"items\":[1]}").unwrap(),
            Sample { items: vec![1] }
        );
        assert!(parse::<Sample>("{\"items\":[").is_err());
    }

    #[tokio::test]
    async fn escreve_atomico_e_apaga_o_tmp_que_sobrou() {
        let dir =
            std::env::temp_dir().join(format!("sollarety-json-{}", crate::store::short_id(8)));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.json");

        assert_eq!(read::<Sample>(&path, "t").await.unwrap(), Sample::default());

        write_atomic(&path, &Sample { items: vec![7] })
            .await
            .unwrap();
        assert!(!temp_path(&path).exists());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\n  \"items\": [\n    7\n  ]\n}"
        );

        std::fs::write(temp_path(&path), "lixo").unwrap();
        delete_leftover_temp(&path, "t");
        assert!(!temp_path(&path).exists());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
