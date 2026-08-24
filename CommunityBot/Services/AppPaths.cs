using System;
using System.IO;

namespace CommunityBot.Services
{
    /// <summary>
    /// Resolve config/ e data/ a partir do diretorio do EXECUTAVEL, nao do
    /// diretorio de trabalho.
    ///
    /// Antes os dois caminhos eram relativos ao cwd, e `dotnet run --project
    /// CommunityBot` NAO troca o diretorio de trabalho - ele fica onde o shell
    /// estava. Rodando o passo 5 do README a partir da raiz do repositorio, o
    /// bot procurava `config/config.jsonc` na raiz, nao achava e saia pelo
    /// caminho de erro fatal. Pior, o `data/warnings.json` ia parar num data/
    /// diferente do que a unit do systemd usa (ela fixa WorkingDirectory), entao
    /// o historico de moderacao de desenvolvimento e o de producao divergiam sem
    /// ninguem perceber.
    /// </summary>
    internal static class AppPaths
    {
        /// <summary>Diretorio onde o assembly esta. Independe de quem chamou o processo.</summary>
        public static string BaseDirectory { get; } = AppContext.BaseDirectory;

        public static string Config(string fileName) =>
            Path.Combine(BaseDirectory, "config", fileName);

        /// <summary>Caminho em data/, criando o diretorio se ainda nao existir.</summary>
        public static string Data(string fileName)
        {
            var directory = Path.Combine(BaseDirectory, "data");
            Directory.CreateDirectory(directory);
            return Path.Combine(directory, fileName);
        }
    }
}
