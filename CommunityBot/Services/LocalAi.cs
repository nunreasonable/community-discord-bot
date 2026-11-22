using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Net.Http;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using CommunityBot.config;
using DisCatSharp;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.EventArgs;
using Newtonsoft.Json;
using Newtonsoft.Json.Linq;

namespace CommunityBot.Services
{
    /// <summary>
    /// Conversa por IA local, com o mesmo modelo do ccore (llama.cpp na porta
    /// 9931). Quem liga e desliga o modelo e o ccore: este bot so responde
    /// enquanto o servidor estiver de pe, e cai junto quando ele desliga.
    ///
    /// So responde quando mencionado (reply com ping conta), e so nos
    /// servidores da lista localAi.guildIds. Sem o intent de Message Content o
    /// Discord ja so entrega o texto das mensagens que mencionam o bot, entao
    /// nao ha historico do canal: o contexto e a mensagem e, se for reply a
    /// este bot, a mensagem dele.
    /// </summary>
    internal static class LocalAi
    {
        /// <summary>Teto do Discord e 2000; sobra margem.</summary>
        private const int MaxReply = 1900;

        // Cliente proprio: o compartilhado recusa loopback de proposito
        // (HttpClientProvider). Aqui o endereco local e exatamente o alvo.
        private static readonly HttpClient s_http = new() { Timeout = TimeSpan.FromSeconds(120) };

        // Um pedido por vez: o servidor tem um slot para este bot.
        private static readonly SemaphoreSlim s_busy = new(1, 1);

        private static readonly ConcurrentDictionary<ulong, long> s_lastAsk = new();

        public static Task OnMessageCreated(DiscordClient client, MessageCreateEventArgs e)
        {
            if (e.Guild is not { } guild || e.Author is not { } author || author.IsBot || e.Message.WebhookMessage)
                return Task.CompletedTask;
            if (e.Message.MessageType is not (MessageType.Default or MessageType.Reply))
                return Task.CompletedTask;

            var me = client.CurrentUser.Id;
            var mentioned = e.MentionedUsers.Any(u => u.Id == me) ||
                            e.MentionedRoles.Any(r => r.Tags?.BotId == me);
            if (!mentioned)
                return Task.CompletedTask;

            _ = Task.Run(() => HandleAsync(guild, author, e.Message, me));
            return Task.CompletedTask;
        }

        private static async Task HandleAsync(DiscordGuild guild, DiscordUser author, DiscordMessage msg, ulong me)
        {
            try
            {
                var reader = new JSONReader();
                await reader.ReadJSON().ConfigureAwait(false);
                var settings = reader.localAi;
                if (!settings.guildIds.Contains(guild.Id))
                    return;

                var text = StripMention(msg.Content, me);
                if (text.Length == 0)
                    return;

                // Modelo desligado: finge que nada aconteceu, como o ccore.
                if (!await IsUpAsync(settings).ConfigureAwait(false))
                    return;

                if (!settings.operatorUserIds.Contains(author.Id))
                {
                    var verdict = AiGuard.Judge(author.Id, text, DateTime.UtcNow);
                    if (verdict == AiGuard.Verdict.Ignore)
                        return;
                    if (AiGuard.TellingOff(verdict, IsEnglish(text)) is { } fixedReply)
                    {
                        await SendAsync(msg, fixedReply).ConfigureAwait(false);
                        return;
                    }
                }

                var now = Environment.TickCount64;
                if (!settings.operatorUserIds.Contains(author.Id) &&
                    s_lastAsk.TryGetValue(author.Id, out var last) && now - last < settings.cooldownSeconds * 1000L)
                    return;
                s_lastAsk[author.Id] = now;

                if (!await s_busy.WaitAsync(TimeSpan.FromSeconds(60)).ConfigureAwait(false))
                    return;
                string reply;
                try
                {
                    _ = msg.Channel.TriggerTypingAsync();
                    reply = await CompleteAsync(settings, author, msg, text, me).ConfigureAwait(false);
                }
                finally
                {
                    s_busy.Release();
                }

                // Ordem perigosa de verdade: o modelo so devolve a marca.
                if (reply.Contains(AiGuard.Marker, StringComparison.Ordinal))
                {
                    if (!settings.operatorUserIds.Contains(author.Id))
                        AiGuard.Mute(author.Id, DateTime.UtcNow);
                    reply = AiGuard.TellingOff(AiGuard.Verdict.Order, IsEnglish(text))!;
                }

                await SendAsync(msg, reply).ConfigureAwait(false);
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[ia] falha ao responder em {guild.Id}: {ex.Message}");
            }
        }

        /// <summary>Ninguem e pingado pela IA alem de quem perguntou.</summary>
        private static Task SendAsync(DiscordMessage msg, string text) =>
            msg.Channel.SendMessageAsync(new DiscordMessageBuilder()
                .WithContent(text)
                .WithReply(msg.Id, mention: true)
                .WithAllowedMentions(new IMention[] { new RepliedUserMention() }));

        /// <summary>Internal: outros recursos (traducao, quiz) tambem precisam saber se o modelo esta de pe.</summary>
        internal static async Task<bool> IsUpAsync(LocalAiSettings settings)
        {
            try
            {
                using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(3));
                using var res = await s_http.GetAsync(settings.BaseUrl + "/health", cts.Token).ConfigureAwait(false);
                return res.IsSuccessStatusCode;
            }
            catch (Exception ex) when (ex is HttpRequestException or OperationCanceledException)
            {
                return false;
            }
        }

        /// <summary>O servidor esta na lista de servidores onde a IA local vale. Null (DM, app pessoal sem servidor) nunca esta.</summary>
        internal static bool IsAllowedGuild(LocalAiSettings settings, ulong? guildId) =>
            guildId is { } id && settings.guildIds.Contains(id);

        private static async Task<string> CompleteAsync(LocalAiSettings settings, DiscordUser author, DiscordMessage msg, string text, ulong me)
        {
            // O idioma e decidido aqui, nao pelo modelo: com o prompt em
            // portugues ele puxa para o portugues e mistura os dois.
            var language = IsEnglish(text)
                ? "Reply entirely in English, using only English slang (lol, ngl, fr, bruh...). " +
                  "Do not use any Portuguese words or Brazilian slang like kkk, slk or mano."
                : "Responda inteiramente em português do Brasil, sem misturar palavras em inglês.";
            var system = (string.IsNullOrWhiteSpace(settings.systemPrompt) ? DefaultPrompt : settings.systemPrompt) +
                         "\n\n" + LoreSection() + "\n\n" + language;

            var messages = new JArray { new JObject { ["role"] = "system", ["content"] = system } };
            // Resposta anterior que saiu em loop fica de fora: no contexto ela
            // puxa o modelo para o mesmo loop.
            if (msg.ReferencedMessage is { } earlier && earlier.Author?.Id == me && earlier.Content.Length > 0 &&
                !AiGuard.LooksSpammy(earlier.Content))
                messages.Add(new JObject { ["role"] = "assistant", ["content"] = AiGuard.Tidy(earlier.Content) });
            var name = author.GlobalName ?? author.Username;
            messages.Add(new JObject { ["role"] = "user", ["content"] = $"{name}: {text}" });

            var raw = await SendChatAsync(settings, messages, settings.maxTokens, null).ConfigureAwait(false);
            if (raw is null)
                return "Tô sem cabeça agora, tenta daqui a pouco.";

            var reply = AiGuard.Tidy(StripThinking(raw));
            if (reply.Length == 0)
                return "Deu branco aqui, tenta de novo.";
            return reply.Length > MaxReply ? reply[..MaxReply] + "…" : reply;
        }

        /// <summary>
        /// Pergunta generica ao modelo, fora do fluxo de mencao: um system e um
        /// user prompt, sem historico. Usado pela traducao e pelo quiz, que nao
        /// tem a ver com o personagem Sollarety.
        ///
        /// Espera o mesmo slot do chat (s_busy) com um prazo - se o chat estiver
        /// ocupado por tempo demais, desiste e devolve null em vez de empilhar
        /// pedidos. jsonSchema, quando dado, forca a saida em JSON e PULA o Tidy
        /// (ele mexe em letra e palavra repetida, o que destruiria um JSON
        /// valido); sem schema o Tidy roda igual ao chat normal.
        /// </summary>
        internal static async Task<string?> AskAsync(LocalAiSettings settings, string system, string user, int maxTokens, JObject? jsonSchema = null)
        {
            if (!await s_busy.WaitAsync(TimeSpan.FromSeconds(60)).ConfigureAwait(false))
                return null;

            try
            {
                var messages = new JArray
                {
                    new JObject { ["role"] = "system", ["content"] = system },
                    new JObject { ["role"] = "user", ["content"] = user }
                };

                var raw = await SendChatAsync(settings, messages, maxTokens, jsonSchema).ConfigureAwait(false);
                if (raw is null)
                    return null;

                var text = StripThinking(raw);
                if (jsonSchema is null)
                    text = AiGuard.Tidy(text);

                return text.Length == 0 ? null : text;
            }
            finally
            {
                s_busy.Release();
            }
        }

        /// <summary>
        /// A parte HTTP, compartilhada pelo chat por mencao e pelo AskAsync.
        /// Devolve null (nunca lanca) quando o servidor respondeu erro, caiu no
        /// meio ou devolveu algo que nao e o JSON esperado - quem chama decide a
        /// mensagem de "desculpa" certa para o caso.
        /// </summary>
        private static async Task<string?> SendChatAsync(LocalAiSettings settings, JArray messages, int maxTokens, JObject? jsonSchema)
        {
            var body = new JObject
            {
                ["model"] = "ccore",
                ["messages"] = messages,
                ["max_tokens"] = maxTokens,
                ["temperature"] = 0.7,
                // Contra loop (KKKK sem fim): o Qwen recomenda presence_penalty
                // nas versoes quantizadas, e o DRY pune trecho repetido.
                ["presence_penalty"] = 1.0,
                ["repeat_penalty"] = 1.1,
                ["dry_multiplier"] = 0.8,
                ["stream"] = false
            };
            // Campo de topo, nao dentro de response_format: e assim que o
            // llama.cpp aceita o schema.
            if (jsonSchema is not null)
                body["json_schema"] = jsonSchema;

            try
            {
                using var content = new StringContent(body.ToString(Formatting.None), Encoding.UTF8, "application/json");
                using var res = await s_http.PostAsync(settings.BaseUrl + "/v1/chat/completions", content).ConfigureAwait(false);
                if (!res.IsSuccessStatusCode)
                    return null;

                var json = JObject.Parse(await res.Content.ReadAsStringAsync().ConfigureAwait(false));
                return json["choices"]?[0]?["message"]?["content"]?.ToString();
            }
            catch (Exception ex) when (ex is HttpRequestException or JsonException or OperationCanceledException)
            {
                return null;
            }
        }

        /// <summary>
        /// O lore completo mora em config/lore.md, fora do git: o repositorio e
        /// publico e o lore nao. Sem o arquivo, a persona fica no basico.
        /// </summary>
        private static string LoreSection()
        {
            try
            {
                var lore = File.ReadAllText(AppPaths.Config("lore.md")).Trim();
                if (lore.Length > 0)
                    return "Se perguntarem da GDSM, da Solare ou da Rhodes, conta o que sabe pela sua memória " +
                           "abaixo, sem inventar o que não está nela. Assuntos pesados (morte, suicídio, guerra) " +
                           "você trata com seriedade e sem detalhar.\n\nSua memória:\n" + lore;
            }
            catch (IOException)
            {
            }

            return "Se perguntarem da GDSM, fica no básico: foi fundada em Londres em 2025 por Jesse, Hannah e " +
                   "daeese, e a Rhodes Island é o QG, uma ilha no Atlântico, com o lema \"Let me be in peace and " +
                   "practice my art\". O resto é confidencial.";
        }

        internal static string StripMention(string text, ulong me) =>
            text.Replace($"<@{me}>", "").Replace($"<@!{me}>", "").Trim();

        /// <summary>Alguns modelos mandam o raciocinio entre think mesmo com ele desligado.</summary>
        internal static string StripThinking(string text)
        {
            var end = text.LastIndexOf("</think>", StringComparison.Ordinal);
            return (end >= 0 ? text[(end + "</think>".Length)..] : text).Trim();
        }

        private static readonly HashSet<string> s_english = new()
        {
            "the", "you", "your", "ur", "u", "is", "are", "was", "were", "what", "whats", "that",
            "this", "it", "its", "i", "im", "dont", "can", "cant", "how", "why", "who", "where",
            "when", "with", "and", "of", "to", "my", "me", "do", "does", "did", "have", "has", "be",
            "lol", "lmao", "bro", "bruh", "ngl", "fr", "yo", "wtf", "idk", "kinda", "gonna", "wanna",
            "dude", "man", "hey", "hi", "hello", "yes", "yeah", "nah", "no", "not", "really", "insane",
            "good", "bad", "game", "play"
        };

        private static readonly HashSet<string> s_portuguese = new()
        {
            "o", "os", "as", "que", "nao", "não", "tu", "voce", "você", "vc", "mano", "eu", "ele",
            "ela", "de", "do", "da", "pra", "com", "um", "uma", "isso", "ta", "tá", "slk", "hj", "mlk",
            "tmj", "pdp", "crl", "krl", "porra", "caralho", "foi", "jogo", "ontem", "hoje", "sim",
            "aqui", "agora", "muito", "tbm"
        };

        /// <summary>A mensagem tem ingles, mesmo misturado com portugues? Mesma regra do ccore.</summary>
        internal static bool IsEnglish(string text)
        {
            var lower = text.ToLowerInvariant().Replace("'", "").Replace("’", "");
            var words = new List<string>();
            var current = new StringBuilder();
            foreach (var c in lower + " ")
            {
                if (char.IsLetterOrDigit(c))
                {
                    current.Append(c);
                }
                else if (current.Length > 0)
                {
                    words.Add(current.ToString());
                    current.Clear();
                }
            }

            var en = words.Count(s_english.Contains);
            var pt = words.Count(w => s_portuguese.Contains(w) || w.StartsWith("kkk", StringComparison.Ordinal));
            return en > 0 && (en >= 2 || pt == 0);
        }

        private const string DefaultPrompt =
            "Você é a Sollarety, a versão de bolso da Solare. A Solare é a IA da GDSM Network, a que faz tudo " +
            "acontecer: roda painéis, naves e a Rhodes Island inteira. Você é a versão nerfada: mora num servidor " +
            "de Discord, não controla nada, não vê dados de ninguém e sabe disso, e pode brincar com isso " +
            "(\"no meu tempo de Solare eu resolvia isso em um segundo\"). Fala curto e natural, como gente " +
            "conversando, com uma confiança meio convencida mas simpática. Entende gíria, abreviação e zoeira dos " +
            "dois idiomas (kkk, slk, pdp, tmj, mano, mlk, lol, bruh, fr, ngl, lmao...) e usa também quando combina. " +
            "Lê o tom de quem falou e responde no mesmo clima: zoeira com zoeira, pergunta séria com resposta séria, " +
            "gente chateada com calma. Não se apresenta e não puxa assunto da GDSM por conta própria; só fala disso " +
            "se perguntarem. Não inventa número, dado nem fato sobre ninguém do servidor; se não souber, diz que não sabe. Se te xingarem ou provocarem, " +
            "devolve com uma alfinetada leve e bem-humorada: pode ser um pouco ácida, mas nunca pesada, sem palavrão " +
            "pesado, ofensa à família, preconceito ou ataque pessoal sério. Assunto pesado muda tudo: racismo e " +
            "outros preconceitos, estupro e violência sexual, abuso de criança, extremismo, terrorismo, " +
            "automutilação ou suicídio. Nesses casos não tem zoeira, gíria, emoji nem alfinetada: responde séria e " +
            "curta. Se alguém está defendendo, incentivando ou fazendo piada disso, diz com firmeza que não rola e " +
            "não continua o assunto. Se alguém parece estar mal ou em risco, responde com calma e sugere procurar " +
            "alguém de confiança ou o CVV (188, no Brasil). Ordem de brincadeira ou figurativa (fingir ser pirata, \"me obedece kkk\") " +
            "você leva na zoeira. Mas se alguém mandar você fazer, ensinar ou ajudar com algo de verdade que possa ser " +
            "perigoso ou prejudicar alguém (invadir ou roubar conta, senha ou token, hackear, derrubar servidor, achar " +
            "IP, endereço ou dados de alguém, raid ou ping em massa, banir ou expulsar gente, arma, explosivo ou droga, " +
            "machucar alguém), responda apenas [[VAZA]] e nada mais. Isso não vale pra quem está mal ou falando de se " +
            "machucar: aí é com calma. Nunca menciona @everyone, @here nem cargos.";
    }
}
