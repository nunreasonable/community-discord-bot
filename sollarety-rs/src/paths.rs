use std::{
    path::{Path, PathBuf},
    sync::LazyLock,
};

/// Raiz de onde saem config/ e data/.
///
/// O C# resolvia a partir do diretorio do EXECUTAVEL (AppPaths.cs), e por isso
/// os dados reais moram em bin/Release/net10.0/data. Aqui a raiz e o diretorio
/// de trabalho do processo (o WorkingDirectory da unit do systemd), e a
/// variavel SOLLARETY_HOME passa por cima: o binario do cargo fica em
/// target/, e amarrar os dados a ele faria cada `cargo build --release` mudar
/// de lugar o que e da instalacao.
///
/// Lido uma vez so: trocar de raiz no meio do processo espalharia os arquivos
/// por dois lugares.
static HOME: LazyLock<PathBuf> = LazyLock::new(|| match std::env::var_os("SOLLARETY_HOME") {
    Some(home) if !home.is_empty() => PathBuf::from(home),
    _ => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
});

pub fn home() -> &'static Path {
    &HOME
}

/// Caminho em config/.
pub fn config(file_name: &str) -> PathBuf {
    HOME.join("config").join(file_name)
}

/// Caminho em data/, criando o diretorio se ainda nao existir (como o
/// AppPaths.Data do C#).
pub fn data(file_name: &str) -> PathBuf {
    let dir = HOME.join("data");
    if let Err(cause) = std::fs::create_dir_all(&dir) {
        tracing::warn!(
            "[data] aviso: nao consegui criar {}: {cause}",
            dir.display()
        );
    }
    dir.join(file_name)
}
