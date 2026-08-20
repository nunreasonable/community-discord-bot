using System;
using System.IO;
using System.Text;
using System.Threading;

namespace CommunityBot.Services
{
    /// <summary>
    /// Duplica a saida do console: repassa tudo para o destino original E
    /// alimenta o <see cref="BotLogBuffer"/>.
    ///
    /// A alternativa seria trocar os ~84 Console.WriteLine por chamadas a um
    /// logger. Um tee foi preferido por tres motivos:
    ///
    /// 1. Nenhum ponto de log existente precisa mudar, entao nao ha risco de um
    ///    deles ficar para tras e sumir do historico.
    /// 2. O stdout continua recebendo exatamente o que recebia, o que preserva o
    ///    `journalctl --user -u ccore-bot` e a interface de terminal.
    /// 3. Pega tambem o que o bot nao escreve de proposito - excecoes nao
    ///    observadas e qualquer coisa que uma dependencia jogue no console.
    /// </summary>
    internal sealed class ConsoleTee : TextWriter
    {
        private readonly TextWriter _inner;
        private readonly StringBuilder _pending = new();
        private readonly object _lock = new();

        private static int s_installed;

        private ConsoleTee(TextWriter inner) => _inner = inner;

        public override Encoding Encoding => _inner.Encoding;

        /// <summary>
        /// Instala o tee em Console.Out e Console.Error. Deve ser a primeira
        /// coisa do Main: qualquer linha escrita antes disto nao entra no
        /// buffer.
        ///
        /// Idempotente pelo mesmo motivo do TerminalShenanigans - instalar duas
        /// vezes empilharia um tee sobre o outro e cada linha seria capturada em
        /// duplicidade.
        /// </summary>
        public static void Install()
        {
            if (Interlocked.Exchange(ref s_installed, 1) == 1)
                return;

            Console.SetOut(new ConsoleTee(Console.Out));
            Console.SetError(new ConsoleTee(Console.Error));
        }

        public override void Write(char value)
        {
            lock (_lock)
            {
                _inner.Write(value);
                Accumulate(value);
            }
        }

        public override void Write(string? value)
        {
            if (value is null)
                return;

            lock (_lock)
            {
                _inner.Write(value);
                Accumulate(value);
            }
        }

        public override void Write(char[] buffer, int index, int count)
        {
            lock (_lock)
            {
                _inner.Write(buffer, index, count);
                for (var i = 0; i < count; i++)
                    Accumulate(buffer[index + i]);
            }
        }

        public override void WriteLine()
        {
            lock (_lock)
            {
                _inner.WriteLine();
                FlushLine();
            }
        }

        public override void WriteLine(string? value)
        {
            lock (_lock)
            {
                _inner.WriteLine(value);
                Accumulate(value ?? string.Empty);
                FlushLine();
            }
        }

        public override void Flush() => _inner.Flush();

        /// <summary>
        /// Acumula texto solto ate encontrar uma quebra de linha. Sem isto, um
        /// Console.Write sem WriteLine (ou os overloads que a base decompoe em
        /// varias chamadas) viraria uma entrada de log por pedaco.
        ///
        /// Chamado sempre com <see cref="_lock"/> ja tomado.
        /// </summary>
        private void Accumulate(string value)
        {
            foreach (var ch in value)
                Accumulate(ch);
        }

        private void Accumulate(char ch)
        {
            if (ch == '\n')
            {
                FlushLine();
                return;
            }

            if (ch == '\r')
                return;

            // Trava de seguranca: sem quebra de linha nenhuma, o acumulador
            // cresceria sem limite. O corte solta o que ja existe como uma
            // linha propria.
            if (_pending.Length >= 4096)
                FlushLine();

            _pending.Append(ch);
        }

        private void FlushLine()
        {
            if (_pending.Length == 0)
                return;

            var line = _pending.ToString();
            _pending.Clear();
            BotLogBuffer.Append(line);
        }
    }
}
