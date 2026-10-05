using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Text;
using System.Threading;
using System.Threading.Tasks;

namespace CommunityBot.Services
{
    /// <summary>
    /// Roda um programa externo (yt-dlp, ffmpeg, git) ate o fim, com prazo.
    ///
    /// Argumentos sempre por ArgumentList: nada de linha de comando montada em
    /// string, que viraria injecao de opcao pelo primeiro titulo de video ou
    /// mensagem de commit com aspas.
    /// </summary>
    internal static class ProcessRunner
    {
        internal readonly record struct ProcessResult(int ExitCode, string StdOut, string StdErr);

        /// <summary>
        /// Estourou o prazo ou cancelou, a arvore inteira morre: o yt-dlp abre o
        /// node (e as vezes o ffmpeg) por baixo, e matar so o pai deixaria os
        /// filhos segurando arquivo.
        /// </summary>
        public static async Task<ProcessResult> RunAsync(string executable, IEnumerable<string> args, TimeSpan timeout,
            CancellationToken ct, IReadOnlyDictionary<string, string>? environment = null)
        {
            var psi = new ProcessStartInfo(executable)
            {
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                UseShellExecute = false,
                CreateNoWindow = true,
                // Explicito: mensagem de commit e titulo de video vem com acento.
                StandardOutputEncoding = Encoding.UTF8,
                StandardErrorEncoding = Encoding.UTF8
            };
            foreach (var arg in args)
                psi.ArgumentList.Add(arg);

            if (environment is not null)
            {
                foreach (var (key, value) in environment)
                    psi.Environment[key] = value;
            }

            using var process = new Process { StartInfo = psi };
            try
            {
                process.Start();
            }
            catch (Win32Exception ex)
            {
                throw new FileNotFoundException($"nao consegui executar '{executable}': {ex.Message}", executable, ex);
            }

            // Os dois fluxos sao lidos em paralelo com a espera: um stderr cheio e
            // nunca lido trava o processo filho no write.
            var stdout = process.StandardOutput.ReadToEndAsync(CancellationToken.None);
            var stderr = process.StandardError.ReadToEndAsync(CancellationToken.None);

            using var deadline = CancellationTokenSource.CreateLinkedTokenSource(ct);
            deadline.CancelAfter(timeout);

            try
            {
                await process.WaitForExitAsync(deadline.Token);
            }
            catch (OperationCanceledException)
            {
                Kill(process);

                // Com o processo morto os pipes fecham e as leituras terminam;
                // espera-las evita excecao nao observada.
                try { await Task.WhenAll(stdout, stderr).WaitAsync(TimeSpan.FromSeconds(5), CancellationToken.None); }
                catch { /* ja se esta saindo por erro */ }

                ct.ThrowIfCancellationRequested();
                throw new TimeoutException($"'{Path.GetFileName(executable)}' passou de {timeout.TotalSeconds:0}s");
            }

            return new ProcessResult(process.ExitCode, await stdout, await stderr);
        }

        public static void Kill(Process process)
        {
            try
            {
                if (!process.HasExited)
                    process.Kill(entireProcessTree: true);
            }
            catch (Exception ex) when (ex is InvalidOperationException or Win32Exception or NotSupportedException)
            {
                // Terminou sozinho entre o HasExited e o Kill.
            }
        }
    }
}
