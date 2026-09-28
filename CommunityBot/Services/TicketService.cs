using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using DisCatSharp;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.EventArgs;

namespace CommunityBot.Services
{
    /// <summary>Config dos tickets ja validado: os dois ids obrigatorios presentes.</summary>
    internal sealed record TicketSettings(ulong CategoryId, ulong StaffRoleId, ulong? LogChannelId);

    /// <summary>O que o fechamento conseguiu fazer. Cada estado pede uma resposta diferente.</summary>
    internal enum CloseOutcome
    {
        /// <summary>Transcript arquivado e canal apagado.</summary>
        Archived,

        /// <summary>Ticket fechado, mas o canal continua de pe - o transcript nao subiu.</summary>
        KeptChannel
    }

    internal sealed record CloseResult(CloseOutcome Outcome, string? Detail);

    /// <summary>
    /// Sistema de tickets: canal privado por assunto, com quem abriu e a equipe
    /// dentro, e um transcript arquivado quando fecha.
    ///
    /// TODO O ESTADO VIVE NO DISCO. O custom id de cada botao carrega o id do
    /// ticket, e o resto sai do <see cref="TicketStore"/> - nao ha dicionario em
    /// memoria em lugar nenhum. E isso que faz um painel publicado continuar
    /// funcionando depois de o bot reiniciar; a mesma razao pela qual o /poll
    /// deixa os votos nas reacoes em vez de no processo.
    ///
    /// ESQUEMA DOS CUSTOM IDS:
    ///   ticket:open:&lt;tipo&gt;   botao do painel        -> abre o modal
    ///   ticket:new:&lt;tipo&gt;    envio daquele modal    -> cria o canal
    ///   ticket:claim:&lt;id&gt;    botao dentro do ticket -> assume o atendimento
    ///   ticket:close:&lt;id&gt;    botao dentro do ticket -> abre o modal de motivo
    ///   ticket:closed:&lt;id&gt;   envio daquele modal    -> fecha de verdade
    /// </summary>
    internal static class TicketService
    {
        public const string Prefix = "ticket:";

        /// <summary>Teto de mensagens no transcript. Acima disso o arquivo vira ilegivel e a coleta, cara.</summary>
        private const int TranscriptLimit = 500;

        /// <summary>O que quem participa de um ticket pode fazer no canal dele.</summary>
        public static readonly Permissions ParticipantPermissions =
            Permissions.AccessChannels | Permissions.SendMessages | Permissions.ReadMessageHistory |
            Permissions.AttachFiles | Permissions.EmbedLinks | Permissions.AddReactions;

        // ------------------------------------------------------------------
        // Config
        // ------------------------------------------------------------------

        /// <summary>
        /// A configuracao de tickets DAQUELE servidor. Devolve null quando o
        /// sistema esta desligado ali - categoria e cargo sao os dois
        /// obrigatorios. Sincrono: le do snapshot em memoria, sem I/O.
        /// </summary>
        public static TicketSettings? TryGetSettings(ulong guildId)
        {
            var settings = GuildSettingsStore.For(guildId);

            var category = settings?.ticketCategoryId ?? 0;
            var staff = settings?.ticketStaffRoleId ?? 0;

            if (category == 0 || staff == 0)
                return null;

            var log = settings!.ticketLogChannelId is > 0 ? settings.ticketLogChannelId : null;
            return new TicketSettings(category, staff, log);
        }

        /// <summary>Embed de recusa quando o sistema nao esta configurado.</summary>
        public static DiscordEmbed NotConfigured() =>
            Embeds.Error("Tickets are off",
                "The ticket system hasn't been set up in this server yet.\n\n" +
                "Someone with **Manage Server** can turn it on with `/config tickets-category` and " +
                "`/config tickets-role`. `/config view` shows what's missing.");

        /// <summary>Diz se o membro faz parte da equipe que atende tickets.</summary>
        public static bool IsStaff(DiscordMember member, TicketSettings settings) =>
            member.Roles.Any(r => r.Id == settings.StaffRoleId) ||
            (member.Permissions & Permissions.Administrator) != 0;

        // ------------------------------------------------------------------
        // Abertura
        // ------------------------------------------------------------------

        /// <summary>
        /// Cria o canal do ticket e registra. Devolve o canal, ou o embed de
        /// recusa quando nao da para abrir.
        ///
        /// A reserva no arquivo acontece ANTES de criar o canal, e num
        /// UpdateAsync so: e la dentro que "um ticket aberto por pessoa" e o
        /// numero sequencial sao decididos, sob o mesmo lock. Conferindo fora,
        /// dois cliques rapidos no botao viram duas tarefas paralelas que passam
        /// as duas pela checagem e abrem dois canais.
        /// </summary>
        public static async Task<(DiscordChannel? Channel, DiscordEmbed? Error)> OpenAsync(
            DiscordClient client, DiscordGuild guild, DiscordMember opener,
            TicketSettings settings, string typeKey, string subject, string? description)
        {
            var type = TicketTypes.Find(typeKey);
            if (type is null)
                return (null, Embeds.Error("Invalid type", "This ticket type no longer exists."));

            if (guild.GetChannel(settings.CategoryId) is not { } category || category.Type != ChannelType.Category)
            {
                return (null, Embeds.Error("Invalid category",
                    "The configured category no longer exists in this server. Set it again with `/config tickets-category`."));
            }

            var staffRole = guild.GetRole(settings.StaffRoleId);
            if (staffRole is null)
            {
                return (null, Embeds.Error("Invalid role",
                    "The configured staff role no longer exists in this server. Set it again with `/config tickets-role`."));
            }

            // EveryoneRole e anulavel com o cache frio - o /lock ja trata o mesmo
            // caso. Sem ele nao da para negar o canal a todo mundo, e um ticket
            // que nasce visivel para o servidor inteiro e pior que ticket nenhum.
            if (guild.EveryoneRole is not { } everyone)
            {
                return (null, Embeds.Error("Role not found",
                    "I couldn't read this server's @everyone role right now. Try again in a few seconds."));
            }

            // Reserva atomica: ou esta pessoa ja tem um ticket aberto, ou sai
            // daqui com numero e id proprios.
            var reserved = await TicketStore.Instance.UpdateAsync(edit =>
            {
                // `channelId != 0` no teste: entre reservar e gravar o id do canal
                // ha uma janela. Se o processo morre ali (deploy, OOM) ou se a
                // segunda escrita falha, sobra um registro aberto apontando para
                // o canal 0 - e sem esta condicao ele bloquearia aquela pessoa de
                // abrir ticket PARA SEMPRE, com um "continue in <#0>" que o
                // Discord mostra como canal apagado. Passados dois minutos, uma
                // reserva sem canal deixa de valer.
                var now = DateTimeOffset.UtcNow;
                var open = edit.File.tickets.FirstOrDefault(t =>
                    t.guildId == guild.Id && t.openerId == opener.Id && t.IsOpen &&
                    (t.channelId != 0 || now - t.openedAtUtc < TimeSpan.FromMinutes(2)));

                if (open is not null)
                    return (Existing: (Ticket?)open, Created: (Ticket?)null);

                var ticket = new Ticket
                {
                    id = TicketStore.NewUniqueId(edit.File),
                    number = TicketStore.NextNumber(edit.File, guild.Id),
                    guildId = guild.Id,
                    openerId = opener.Id,
                    type = type.Key,
                    subject = Embeds.Trim(subject, 200),
                    openedAtUtc = DateTimeOffset.UtcNow
                };

                edit.File.tickets.Add(ticket);
                edit.MarkChanged();
                return (Existing: (Ticket?)null, Created: (Ticket?)ticket);
            });

            if (reserved.Existing is not null)
            {
                return (null, Embeds.Error("You already have an open ticket",
                    $"Continue in <#{reserved.Existing.channelId}>. Close it before opening another one."));
            }

            var created = reserved.Created!;
            var name = $"{type.ChannelPrefix}-{created.number:0000}";

            DiscordChannel channel;
            try
            {
                var overwrites = new List<DiscordOverwriteBuilder>
                {
                    new DiscordOverwriteBuilder(everyone).Deny(Permissions.AccessChannels),
                    new DiscordOverwriteBuilder(opener).Allow(ParticipantPermissions),
                    new DiscordOverwriteBuilder(staffRole).Allow(ParticipantPermissions)
                };

                // O proprio bot, explicitamente: se a categoria negar o canal ao
                // bot, o ticket nasceria inacessivel para quem tem de fecha-lo.
                if (guild.CurrentMember is { } botMember)
                    overwrites.Add(new DiscordOverwriteBuilder(botMember)
                        .Allow(ParticipantPermissions | Permissions.ManageChannels | Permissions.ManageMessages));

                channel = await guild.CreateTextChannelAsync(
                    name,
                    category,
                    // O topic e o encosto para o caso de o tickets.json se perder:
                    // da para reconstruir a mao de quem e o canal.
                    $"Ticket {created.id} — opened by {opener.UsernameWithDiscriminator} ({opener.Id})",
                    overwrites,
                    reason: AuditReason.For(opener.UsernameWithDiscriminator, $"opened ticket {created.id}"));
            }
            catch (Exception ex)
            {
                // O canal nao existe, entao o registro reservado nao pode ficar:
                // ele bloquearia a pessoa de abrir outro, apontando para um canal
                // que nunca nasceu. O try e porque isto roda DENTRO de um catch -
                // uma excecao aqui substituiria o erro original por um de disco e
                // ainda deixaria a reserva de pe.
                try
                {
                    await ForgetAsync(created.id);
                }
                catch (Exception inner)
                {
                    Console.WriteLine($"[tickets] nao consegui limpar a reserva {created.id}: {inner.Message}");
                }


                Console.WriteLine($"[tickets] falha ao criar o canal de {opener.Id}: {ex.Message}");
                return (null, Embeds.Error("Couldn't open the ticket", Embeds.Trim(ex.Message, 500)));
            }

            try
            {
                await TicketStore.Instance.UpdateAsync(edit =>
                {
                    var stored = edit.File.tickets.FirstOrDefault(t => t.id == created.id);
                    if (stored is null)
                        return false;

                    stored.channelId = channel.Id;
                    edit.MarkChanged();
                    return true;
                });
            }
            catch (Exception ex)
            {
                // O canal existe mas o registro nao sabe o id dele - ninguem
                // conseguiria fechar por ali. Desfaz o canal em vez de deixar um
                // orfao com quem abriu bloqueado atras dele.
                Console.WriteLine($"[tickets] canal {channel.Id} criado, mas nao consegui gravar o id: {ex.Message}");

                try
                {
                    await channel.DeleteAsync(AuditReason.Automatic($"ticket {created.id}: failed to register"));
                }
                catch (Exception inner)
                {
                    Console.WriteLine($"[tickets] e nem consegui apagar {channel.Id}: {inner.Message}");
                }

                await ForgetAsync(created.id);
                return (null, Embeds.Error("Couldn't open the ticket",
                    "I couldn't register the ticket. Try again in a few seconds."));
            }

            try
            {
                await channel.SendMessageAsync(OpeningMessage(created, type, opener, staffRole, description));
            }
            catch (Exception ex)
            {
                // O canal existe e o ticket esta registrado; so a mensagem de
                // abertura falhou. Nao e motivo para desfazer nada.
                Console.WriteLine($"[tickets] canal {channel.Id} criado, mas a mensagem de abertura falhou: {ex.Message}");
            }

            await LogAsync(client, guild, settings, Embeds.Info($"Ticket #{created.number:0000} opened",
                $"{opener.Mention} opened a **{type.Label}** ticket in {channel.Mention}.\n" +
                $"**Subject:** {Embeds.Safe(created.subject)}"));

            return (channel, null);
        }

        private static DiscordMessageBuilder OpeningMessage(
            Ticket ticket, TicketType type, DiscordMember opener, DiscordRole staffRole, string? description)
        {
            var embed = new DiscordEmbedBuilder()
                .WithTitle($"{type.Emoji} Ticket #{ticket.number:0000} — {type.Label}")
                .WithDescription(
                    $"Opened by {opener.Mention}.\n\n" +
                    $"**Subject:** {Embeds.Safe(ticket.subject)}" +
                    (string.IsNullOrWhiteSpace(description)
                        ? string.Empty
                        : $"\n\n{Embeds.SafeTrim(description, 1500)}"))
                .WithColor(DiscordColor.Blurple)
                .WithFooter($"id {ticket.id}")
                .WithTimestamp(ticket.openedAtUtc);

            return new DiscordMessageBuilder()
                // A mencao ao cargo vai no CONTENT: embed nao notifica ninguem, e
                // um ticket que a equipe so descobre por acaso nao serve.
                .WithContent(staffRole.Mention)
                .AddEmbed(embed)
                .AddComponents(
                    new DiscordButtonComponent(ButtonStyle.Secondary, $"{Prefix}claim:{ticket.id}", "Claim",
                        emoji: new DiscordComponentEmoji("🙋")),
                    new DiscordButtonComponent(ButtonStyle.Danger, $"{Prefix}close:{ticket.id}", "Close",
                        emoji: new DiscordComponentEmoji("🔒")));
        }

        /// <summary>Apaga um registro reservado cujo canal nunca chegou a existir.</summary>
        private static Task ForgetAsync(string ticketId) =>
            TicketStore.Instance.UpdateAsync(edit =>
            {
                var stored = edit.File.tickets.FirstOrDefault(t => t.id == ticketId);
                if (stored is null)
                    return false;

                edit.File.tickets.Remove(stored);
                edit.MarkChanged();
                return true;
            });

        // ------------------------------------------------------------------
        // Fechamento
        // ------------------------------------------------------------------

        /// <summary>
        /// Arquiva e fecha.
        ///
        /// A ORDEM NAO E NEGOCIAVEL: o transcript sobe primeiro, e so depois o
        /// canal e apagado. Se o arquivamento falhar, o canal FICA - apagar mesmo
        /// assim destruiria a conversa inteira sem deixar nada no lugar, que e o
        /// oposto do que arquivar quer dizer. Sem canal de log configurado nao ha
        /// arquivamento possivel, e o mesmo raciocinio vale: tranca e avisa.
        /// </summary>
        public static async Task<CloseResult> CloseAsync(
            DiscordClient client, DiscordGuild guild, DiscordChannel channel,
            Ticket ticket, DiscordUser closedBy, string? reason, TicketSettings settings)
        {
            // O canal tem de ser o do ticket. Os dois chamadores passam o canal da
            // interacao, entao hoje bate sempre - e justamente por isso a
            // divergencia so apareceria como um canal errado apagado.
            if (channel.Id != ticket.channelId)
            {
                Console.WriteLine($"[tickets] recusado: pedido de fechar o ticket {ticket.id} " +
                                  $"(canal {ticket.channelId}) veio do canal {channel.Id}.");
                return new CloseResult(CloseOutcome.KeptChannel,
                    "This channel doesn't belong to that ticket.");
            }

            // RESERVA, e nao fechamento. O resultado E conferido: descartar o bool
            // aqui deixava dois cliques simultaneos rodarem o fechamento inteiro
            // em paralelo - dois transcripts no log, e o segundo DeleteAsync
            // batendo num canal ja apagado, o que virava um "couldn't delete"
            // assustador para um fechamento que deu certo.
            var reserved = await TicketStore.Instance.UpdateAsync(edit =>
            {
                var now = DateTimeOffset.UtcNow;
                var stored = edit.File.tickets.FirstOrDefault(t => t.id == ticket.id);

                if (stored is null || !stored.IsOpen || stored.IsClosing(now))
                    return false;

                stored.closingSince = now;
                edit.MarkChanged();
                return true;
            });

            if (!reserved)
            {
                return new CloseResult(CloseOutcome.KeptChannel,
                    "Someone else is already closing this ticket, or it has already been closed.");
            }

            if (settings.LogChannelId is null)
            {
                await ReleaseClosingAsync(ticket.id);
                await LockAsync(guild, channel, ticket, closedBy);
                return new CloseResult(CloseOutcome.KeptChannel,
                    "No ticket log channel is set up (`/config tickets-log`), so there was nowhere to archive the conversation. " +
                    "The channel was locked and has to be deleted manually.");
            }

            string transcript;
            int messageCount;
            try
            {
                (transcript, messageCount) = await BuildTranscriptAsync(channel, ticket);
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[tickets] falha ao montar o transcript de {ticket.id}: {ex.Message}");
                await ReleaseClosingAsync(ticket.id);
                await LockAsync(guild, channel, ticket, closedBy);
                return new CloseResult(CloseOutcome.KeptChannel,
                    $"I couldn't read the messages to archive them: {Embeds.Trim(ex.Message, 300)}. " +
                    "The channel was locked instead of deleted.");
            }

            try
            {
                var log = await client.GetChannelAsync(settings.LogChannelId.Value);
                if (log is null || log.GuildId != guild.Id)
                    throw new InvalidOperationException("the log channel isn't in this server");

                var embed = new DiscordEmbedBuilder()
                    .WithTitle($"Ticket #{ticket.number:0000} closed — {TicketTypes.Describe(ticket.type)}")
                    .WithColor(DiscordColor.Orange)
                    .WithTimestamp(DateTimeOffset.UtcNow)
                    .AddField(new DiscordEmbedField("Opened by", $"<@{ticket.openerId}>\n`{ticket.openerId}`", true))
                    .AddField(new DiscordEmbedField("Closed by", $"{closedBy.Mention}\n`{closedBy.Id}`", true))
                    .AddField(new DiscordEmbedField("Claimed by",
                        ticket.claimedById is { } c ? $"<@{c}>" : "*nobody*", true))
                    .AddField(new DiscordEmbedField("Subject", Embeds.SafeTrim(ticket.subject, 900), false))
                    .AddField(new DiscordEmbedField("Closing reason",
                        string.IsNullOrWhiteSpace(reason) ? "*not provided*" : Embeds.SafeTrim(reason, 900), false))
                    .AddField(new DiscordEmbedField("Duration", DescribeDuration(ticket), true))
                    .AddField(new DiscordEmbedField("Messages", messageCount.ToString(CultureInfo.InvariantCulture), true))
                    .WithFooter($"id {ticket.id}");

                using var stream = new MemoryStream(Encoding.UTF8.GetBytes(transcript));
                await log.SendMessageAsync(new DiscordMessageBuilder()
                    .AddEmbed(embed)
                    .AddFile($"ticket-{ticket.number:0000}-{ticket.id}.txt", stream));
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[tickets] falha ao arquivar o ticket {ticket.id}: {ex.Message}");
                await ReleaseClosingAsync(ticket.id);
                await LockAsync(guild, channel, ticket, closedBy);
                return new CloseResult(CloseOutcome.KeptChannel,
                    $"I couldn't send the transcript to the log channel: {Embeds.Trim(ex.Message, 300)}. " +
                    "The channel was locked instead of deleted so the conversation isn't lost.");
            }

            // A conversa esta salva: agora sim o ticket esta fechado. Antes deste
            // ponto qualquer falha devolve um ticket ABERTO, que da para tentar
            // fechar de novo.
            await TicketStore.Instance.UpdateAsync(edit =>
            {
                var stored = edit.File.tickets.FirstOrDefault(t => t.id == ticket.id);
                if (stored is null)
                    return false;

                stored.closedAtUtc = DateTimeOffset.UtcNow;
                stored.closedById = closedBy.Id;
                stored.closingSince = null;
                edit.MarkChanged();
                return true;
            });

            try
            {
                await channel.DeleteAsync(AuditReason.For(closedBy.UsernameWithDiscriminator,
                    $"closed ticket {ticket.id}"));
            }
            catch (Exception ex)
            {
                // O transcript esta no log e o ticket esta fechado; so o canal
                // sobrou. Trancar e obrigatorio: sem isso, gente continua
                // conversando num canal que o bot ja arquivou e nao reconhece
                // mais, e essa conversa se perde quando alguem o apagar.
                Console.WriteLine($"[tickets] transcript salvo, mas nao consegui apagar {channel.Id}: {ex.Message}");
                await LockAsync(guild, channel, ticket, closedBy);
                return new CloseResult(CloseOutcome.KeptChannel,
                    "The transcript was archived and the ticket is closed, but I couldn't delete the channel: " +
                    $"{Embeds.Trim(ex.Message, 300)}. Delete it manually.");
            }

            return new CloseResult(CloseOutcome.Archived, null);
        }

        /// <summary>
        /// Solta a reserva de fechamento. Chamado em toda saida que deixa o canal
        /// de pe: o ticket volta a ser um ticket ABERTO, e da para tentar fechar
        /// de novo depois de arrumar o que falhou.
        /// </summary>
        private static Task ReleaseClosingAsync(string ticketId) =>
            TicketStore.Instance.UpdateAsync(edit =>
            {
                var stored = edit.File.tickets.FirstOrDefault(t => t.id == ticketId);
                if (stored is null || stored.closingSince is null)
                    return false;

                stored.closingSince = null;
                edit.MarkChanged();
                return true;
            });

        /// <summary>
        /// Tranca o canal quando nao deu para arquivar. Preserva o resto do
        /// overwrite de quem abriu, mexendo so no bit de escrita - o mesmo
        /// cuidado do /lock.
        /// </summary>
        private static async Task LockAsync(DiscordGuild guild, DiscordChannel channel, Ticket ticket, DiscordUser closedBy)
        {
            try
            {
                var reason = AuditReason.For(closedBy.UsernameWithDiscriminator, $"ticket {ticket.id} closed");
                var opener = await Hierarchy.TryGetMemberAsync(guild, ticket.openerId);

                if (opener is not null)
                {
                    var existing = channel.PermissionOverwrites.FirstOrDefault(o => o.Id == opener.Id);
                    var allow = (existing?.Allowed ?? Permissions.None) & ~Permissions.SendMessages;
                    var deny = (existing?.Denied ?? Permissions.None) | Permissions.SendMessages;

                    await channel.AddOverwriteAsync(opener, allow, deny, reason);
                }

                // O @everyone tambem, sempre. Trancar so quem abriu deixava o
                // canal aberto para todo mundo que tivesse sido chamado com
                // /ticket-add - e, se quem abriu ja tinha saido do servidor,
                // o GetMemberAsync lancava e o canal nao era trancado DE JEITO
                // NENHUM, que era o pior dos casos.
                if (guild.EveryoneRole is { } everyone)
                {
                    var current = channel.PermissionOverwrites.FirstOrDefault(o => o.Id == everyone.Id);
                    var allow = (current?.Allowed ?? Permissions.None) & ~Permissions.SendMessages;
                    var deny = (current?.Denied ?? Permissions.None) | Permissions.SendMessages;

                    await channel.AddOverwriteAsync(everyone, allow, deny, reason);
                }
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[tickets] nao consegui trancar {channel.Id}: {ex.Message}");
            }
        }

        /// <summary>
        /// Monta o transcript em ordem cronologica. Pagina para tras a partir da
        /// mensagem mais nova, com teto: um canal de ticket que virou conversa de
        /// mil mensagens nao pode transformar um clique em minutos de REST.
        /// </summary>
        private static async Task<(string Text, int Count)> BuildTranscriptAsync(DiscordChannel channel, Ticket ticket)
        {
            var collected = new List<DiscordMessage>();
            var batch = await channel.GetMessagesAsync(100);
            var truncated = false;

            while (batch.Count > 0)
            {
                collected.AddRange(batch);

                // Fim do canal: nao ha mais o que buscar.
                if (batch.Count < 100)
                    break;

                // Bateu no teto E ainda havia mais. O `collected.Count >= limite`
                // sozinho, como condicao do while, nunca deixava a contagem
                // ULTRAPASSAR o teto - ela chegava exatamente a 500 -, entao o
                // aviso de truncagem la embaixo era codigo morto e um ticket de
                // cinco mil mensagens virava um transcript cortado em silencio.
                if (collected.Count >= TranscriptLimit)
                {
                    truncated = true;
                    break;
                }

                batch = await channel.GetMessagesBeforeAsync(batch[^1].Id, 100);
            }

            var ordered = collected
                .Take(TranscriptLimit)
                .OrderBy(m => m.CreationTimestamp)
                .ToList();

            var sb = new StringBuilder();
            sb.AppendLine($"Ticket #{ticket.number:0000} ({ticket.id}) — {TicketTypes.Describe(ticket.type)}");
            sb.AppendLine($"Subject: {ticket.subject}");
            sb.AppendLine($"Opened by: {ticket.openerId}");
            sb.AppendLine($"Opened at: {ticket.openedAtUtc:yyyy-MM-dd HH:mm:ss} UTC");
            sb.AppendLine($"Closed at: {DateTimeOffset.UtcNow:yyyy-MM-dd HH:mm:ss} UTC");
            if (truncated)
                sb.AppendLine($"WARNING: only the {TranscriptLimit} most recent messages are included in this file.");
            sb.AppendLine(new string('-', 72));
            sb.AppendLine();

            foreach (var m in ordered)
            {
                sb.AppendLine($"[{m.CreationTimestamp:yyyy-MM-dd HH:mm:ss}] {m.Author?.UsernameWithDiscriminator ?? "?"} ({m.Author?.Id}):");

                // Mensagem sem texto (so anexo, so embed, so figurinha) e comum.
                //
                // Nao tem a ver com o intent MessageContent: este e um GET REST em
                // /channels/{id}/messages, e aquele intent so restringe o que o
                // GATEWAY entrega. O conteudo vem completo aqui.
                sb.AppendLine(string.IsNullOrWhiteSpace(m.Content)
                    ? "    (no text content)"
                    : "    " + m.Content.Replace("\n", "\n    "));

                foreach (var a in m.Attachments)
                    sb.AppendLine($"    [attachment] {a.Filename} — {a.Url}");

                foreach (var e in m.Embeds)
                    sb.AppendLine($"    [embed] {e.Title}");

                sb.AppendLine();
            }

            return (sb.ToString(), ordered.Count);
        }

        private static string DescribeDuration(Ticket ticket)
        {
            var span = (ticket.closedAtUtc ?? DateTimeOffset.UtcNow) - ticket.openedAtUtc;
            return DurationParser.Describe(span);
        }

        // ------------------------------------------------------------------
        // Diagnostico de subida
        // ------------------------------------------------------------------

        /*
         * Sem isto, cada jeito de errar a configuracao produz o MESMO sintoma:
         * /ticket recusa e ninguem sabe qual das cinco coisas esta errada.
         *
         * Junta TODOS os problemas numa lista em vez de parar no primeiro - a
         * licao do vigia de canal: quem arruma um, reinicia e descobre o
         * seguinte, um de cada vez.
         *
         * Roda no GuildDownloadCompleted, que re-dispara a cada reconexao, entao
         * o relatorio so e escrito quando MUDA.
         */
        private static string? s_lastReport;

        public static async Task ReportStatusAsync(DiscordClient client, GuildDownloadCompletedEventArgs e)
        {
            try
            {
                await ReconcileAsync(client);

                var report = BuildStatusReportAsync(client);
                if (report == Volatile.Read(ref s_lastReport))
                    return;

                Volatile.Write(ref s_lastReport, report);
                Console.WriteLine(report);
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[tickets] falha ao conferir a configuracao: {ex.Message}");
            }
        }

        /// <summary>
        /// Fecha registros de ticket cujo canal nao existe mais.
        ///
        /// Sem isto ha duas maneiras de uma pessoa ficar PERMANENTEMENTE impedida
        /// de abrir ticket, as duas invisiveis:
        ///
        ///   1. Um administrador apaga o canal do ticket na mao, ou apaga a
        ///      categoria inteira numa reorganizacao. O registro continua aberto
        ///      para sempre.
        ///   2. O processo morre entre criar o canal e gravar o id dele, e sobra
        ///      um registro aberto com channelId 0.
        ///
        /// Nos dois casos a proxima tentativa daquela pessoa bate no "You already
        /// have an open ticket" apontando para um canal que nao existe, e a unica
        /// saida era editar o tickets.json a mao.
        ///
        /// Roda na subida, que e quando o cache de canais esta completo.
        /// </summary>
        private static async Task ReconcileAsync(DiscordClient client)
        {
            var closed = await TicketStore.Instance.UpdateAsync(edit =>
            {
                var orphans = new List<Ticket>();

                foreach (var ticket in edit.File.tickets.Where(t => t.IsOpen))
                {
                    // Servidor fora do cache: pode ser indisponibilidade
                    // passageira do Discord, e nao um servidor que sumiu. Nao
                    // mexe - fechar por engano perderia o ticket de alguem.
                    if (!client.Guilds.TryGetValue(ticket.guildId, out var guild))
                        continue;

                    if (ticket.channelId != 0 && guild.GetChannel(ticket.channelId) is not null)
                        continue;

                    // Reserva ainda dentro da janela: outra tarefa pode estar
                    // criando o canal neste instante.
                    if (ticket.channelId == 0 &&
                        DateTimeOffset.UtcNow - ticket.openedAtUtc < TimeSpan.FromMinutes(2))
                        continue;

                    ticket.closedAtUtc = DateTimeOffset.UtcNow;
                    ticket.closingSince = null;
                    orphans.Add(ticket);
                }

                if (orphans.Count > 0)
                    edit.MarkChanged();

                return orphans;
            });

            foreach (var ticket in closed)
                Console.WriteLine($"[tickets] ticket {ticket.id} (#{ticket.number:0000}) fechado na subida: " +
                                  $"o canal {ticket.channelId} nao existe mais.");
        }

        private static string BuildStatusReportAsync(DiscordClient client)
        {
            // Um relatorio por servidor configurado. A busca reversa que morava
            // aqui - "de qual servidor e esta categoria?" - so existia porque o
            // config nao tinha chave de servidor; agora o servidor E a chave.
            var lines = client.Guilds.Values
                .Select(g => (Guild: g, Settings: TryGetSettings(g.Id)))
                .Where(x => x.Settings is not null)
                .Select(x => DescribeGuild(x.Guild, x.Settings!))
                .ToList();

            return lines.Count == 0
                ? "[tickets] sistema de tickets desligado em todos os servidores (use /config tickets-category e /config tickets-role)."
                : string.Join("\n", lines);
        }

        private static string DescribeGuild(DiscordGuild guild, TicketSettings settings)
        {
            var problems = new List<string>();
            var category = guild.GetChannel(settings.CategoryId);

            if (category!.Type != ChannelType.Category)
                problems.Add($"ticketCategoryId aponta para um canal do tipo {category.Type}, e nao para uma CATEGORIA");

            var staffRole = guild.GetRole(settings.StaffRoleId);
            if (staffRole is null)
            {
                problems.Add($"o cargo {settings.StaffRoleId} nao existe neste servidor");
            }
            else if (!staffRole.IsMentionable)
            {
                // A mensagem de abertura poe a mencao ao cargo no content
                // justamente para NOTIFICAR a equipe. Com o cargo nao mencionavel
                // ela vira texto inerte e ninguem e avisado - o ticket fica
                // esperando alguem passar por ali por acaso.
                problems.Add($"o cargo \"{staffRole.Name}\" nao e mencionavel, entao a equipe " +
                             "nao recebe notificacao quando um ticket abre");
            }

            var bot = guild.CurrentMember;
            if (bot is null)
            {
                problems.Add("nao consegui ler o meu proprio membro para conferir permissoes");
            }
            else
            {
                if ((bot.Permissions & Permissions.ManageChannels) == 0)
                    problems.Add("estou sem Manage Channels, entao nao consigo criar nem apagar o canal do ticket");

                if ((bot.Permissions & Permissions.ManageRoles) == 0)
                    problems.Add("estou sem Manage Roles, e sem isso nao da para escrever as permissoes do canal");
            }

            if (settings.LogChannelId is null)
            {
                problems.Add("nao ha canal de log (/config tickets-log), entao o fechamento vai trancar o canal em vez de arquivar e apagar");
            }
            else
            {
                var log = guild.GetChannel(settings.LogChannelId.Value);
                if (log is null)
                    problems.Add($"o canal de log {settings.LogChannelId} nao esta neste servidor");
                else if (bot is not null && (log.PermissionsFor(bot) & Permissions.AttachFiles) == 0)
                    problems.Add($"nao posso anexar arquivo em \"{log.Name}\", e o transcript e um anexo");
            }

            if (problems.Count == 0)
                return $"[tickets] ligado em \"{guild.Name}\": categoria \"{category.Name}\", " +
                       $"{TicketTypes.All.Count} tipo(s) de ticket.";

            return $"[tickets] AVISO: configurado em \"{guild.Name}\", mas: {string.Join("; ", problems)}.";
        }

        // ------------------------------------------------------------------
        // Log
        // ------------------------------------------------------------------

        /// <summary>
        /// Manda um embed ao canal de log dos tickets. Nunca lanca, e confere o
        /// servidor: o bot esta em mais de um, e GetChannelAsync resolve id de
        /// qualquer um deles - publicar num canal de outro servidor entregaria
        /// conversa privada a quem nao tem nada com aquilo.
        /// </summary>
        public static async Task LogAsync(DiscordClient client, DiscordGuild guild, TicketSettings settings, DiscordEmbed embed)
        {
            if (settings.LogChannelId is null)
                return;

            try
            {
                var channel = await client.GetChannelAsync(settings.LogChannelId.Value);
                if (channel is null)
                    return;

                if (channel.GuildId != guild.Id)
                {
                    Console.WriteLine("[tickets] o ticketLogChannelId configurado nao e deste servidor; nada registrado.");
                    return;
                }

                await channel.SendMessageAsync(new DiscordMessageBuilder().AddEmbed(embed));
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[tickets] falha ao registrar no canal de log: {ex.Message}");
            }
        }
    }
}
