using System;
using System.Collections.Generic;
using System.Linq;
using System.Threading.Tasks;
using CommunityBot.Services;
using CommunityBot.Services.Roblox;
using DisCatSharp.ApplicationCommands;
using DisCatSharp.ApplicationCommands.Attributes;
using DisCatSharp.ApplicationCommands.Context;
using DisCatSharp.Entities;
using DisCatSharp.Enums;

namespace CommunityBot.commands
{
    /// <summary>
    /// Configuracao do bot, por servidor, feita por quem manda no servidor.
    ///
    /// Antes tudo isto vivia num arquivo na maquina do bot, o que exigia acesso
    /// SSH para apontar um canal - e, pior, as chaves eram globais do processo:
    /// com o bot em mais de um servidor, o log de moderacao de um podia cair no
    /// canal de outro.
    ///
    /// Manage Server e o portao, mesmo criterio do /ticket-panel. O dono sempre
    /// a tem e pode delegar sem entregar a conta.
    ///
    /// Cada subcomando mexe em UMA chave e tem UMA opcao opcional, onde omitir
    /// significa limpar. Um comando so com varias opcoes opcionais seria mais
    /// curto e ambiguo: nao daria para distinguir "nao mexa nisso" de "limpe
    /// isso".
    /// </summary>
    [SlashCommandGroup("config", "Configures the bot for this server", (long)Permissions.ManageGuild, allowedContexts: new[] { InteractionContextType.Guild })]
    [ApplicationCommandRequireGuild]
    [ApplicationCommandRequireUserPermissions(Permissions.ManageGuild)]
    internal class Config : ApplicationCommandsModule
    {
        [SlashCommand("view", "Shows this server's current configuration")]
        public async Task ShowCommand(InteractionContext ctx)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            var guild = ctx.Guild!;
            var settings = GuildSettingsStore.For(guild.Id);

            var embed = new DiscordEmbedBuilder()
                .WithTitle($"Configuration for {guild.Name}")
                .WithColor(DiscordColor.Blurple)
                .AddField(new DiscordEmbedField("Moderation log",
                    Describe(guild, settings?.moderationLogChannelId, "`/config mod-log`"), false))
                .AddField(new DiscordEmbedField("Auto-softban",
                    Describe(guild, settings?.autoSoftbanChannelId, "`/config auto-softban`"), false))
                .AddField(new DiscordEmbedField("Tickets — category",
                    Describe(guild, settings?.ticketCategoryId, "`/config tickets-category`"), true))
                .AddField(new DiscordEmbedField("Tickets — staff role",
                    settings?.ticketStaffRoleId is { } r && guild.GetRole(r) is { } role
                        ? role.Mention
                        : settings?.ticketStaffRoleId is not null
                            ? $"⚠️ role `{settings.ticketStaffRoleId}` no longer exists"
                            : "— *(`/config tickets-role`)*", true))
                .AddField(new DiscordEmbedField("Tickets — log channel",
                    Describe(guild, settings?.ticketLogChannelId, "`/config tickets-log`"), true))
                .AddField(new DiscordEmbedField("Verification — role",
                    DescribeRole(guild, settings?.verifiedRoleId, "`/config verify-role`"), true))
                .AddField(new DiscordEmbedField("Verification — unverified",
                    DescribeRole(guild, settings?.unverifiedRoleId, "`/config verify-unverified-role`"), true))
                .AddField(new DiscordEmbedField("Verification — nickname",
                    NicknameFormats.Describe(settings?.nicknameFormat), true))
                .AddField(new DiscordEmbedField("Verification — minimum age",
                    settings?.minAccountAgeDays is > 0 and var days ? $"{days} day(s)" : "off", true))
                .AddField(new DiscordEmbedField("Group binds",
                    settings?.groupBinds is { Count: > 0 } binds ? $"{binds.Count} (`/bind list`)" : "— *(`/bind add`)*", true))
                .AddField(new DiscordEmbedField("Levels",
                    settings?.levelingEnabled == true ? "on" : "off *(`/config leveling`)*", true))
                .AddField(new DiscordEmbedField("Economy",
                    settings?.economyEnabled == true ? "on" : "off *(`/config economy`)*", true));

            var verifyReady = (await VerificationFlow.ReadSettingsAsync()).IsConfigured;
            var pending = Pending(guild, settings, verifyReady);
            if (pending.Count > 0)
                embed.AddField(new DiscordEmbedField("Still missing",
                    string.Join("\n", pending.Select(p => $"• {p}")), false));

            if (settings?.updatedAtUtc is { } when)
                embed.WithFooter($"Last changed by {settings.updatedById} on {when:yyyy-MM-dd HH:mm} UTC");

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(embed));
        }

        [SlashCommand("mod-log", "Channel that gets a record of every moderation action")]
        public Task ModLogCommand(
            InteractionContext ctx,
            [ChannelTypes(ChannelType.Text, ChannelType.News)]
            [Option("channel", "Leave empty to turn logging off")] DiscordChannel? canal = null) =>
            ApplyAsync(ctx, canal,
                needs: Permissions.SendMessages | Permissions.EmbedLinks,
                assign: (s, v) => s.moderationLogChannelId = v,
                onSet: c => Embeds.Ok("Moderation log on",
                    $"Moderation actions will now be logged in {c.Mention}."),
                onClear: () => Embeds.Ok("Moderation log off",
                    "Nothing will be logged beyond Discord's own Audit Log."));

        [SlashCommand("auto-softban", "Trap channel: anyone who posts in it is softbanned automatically")]
        public Task AutoSoftbanCommand(
            InteractionContext ctx,
            [Option("channel", "Leave empty to disarm the trap")] DiscordChannel? canal = null) =>
            ApplyAsync(ctx, canal,
                needs: Permissions.AccessChannels,
                assign: (s, v) => s.autoSoftbanChannelId = v,
                // Este comando arma um recurso que bane sozinho. A resposta diz
                // exatamente o que vai acontecer, em vez de um "ok" de uma linha.
                onSet: c => Embeds.Ok("Trap armed",
                    $"**Any message in {c.Mention} now results in an automatic softban** — " +
                    "a ban followed by an unban, which deletes the person's last 7 days of messages and leaves them " +
                    "free to come back with an invite.\n\n" +
                    "Exempt: bots and webhooks, system messages, anyone with Ban Members, " +
                    "Manage Server or Administrator, the owner, and anyone above my role. " +
                    "Forum posts count, since they are threads inside the channel.\n\n" +
                    "After more than 5 softbans in 60 seconds, the watcher disarms itself until the bot restarts."),
                onClear: () => Embeds.Ok("Trap disarmed",
                    "No one will be punished automatically for posting in any channel anymore."));

        [SlashCommand("tickets-category", "Category where ticket channels are created")]
        public Task TicketCategoryCommand(
            InteractionContext ctx,
            [ChannelTypes(ChannelType.Category)]
            [Option("category", "Leave empty to turn tickets off")] DiscordChannel? categoria = null) =>
            ApplyAsync(ctx, categoria,
                needs: Permissions.None,
                assign: (s, v) => s.ticketCategoryId = v,
                onSet: c => Embeds.Ok("Ticket category set",
                    $"Ticket channels will be created in **{c.Name}**. Check the rest with `/config view`."),
                onClear: () => Embeds.Ok("Tickets off",
                    "Without a category there is nowhere to create the channels, so `/ticket` will refuse."));

        [SlashCommand("tickets-role", "Staff role that can see every ticket")]
        public async Task TicketRoleCommand(
            InteractionContext ctx,
            [Option("role", "Leave empty to turn tickets off")] DiscordRole? cargo = null)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            await GuildSettingsStore.Instance.SetAsync(ctx.Guild!.Id, ctx.User.Id,
                s => s.ticketStaffRoleId = cargo?.Id);

            if (cargo is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Ok("Tickets off",
                        "Without a staff role there is no one to give access to, so `/ticket` will refuse.")));
                return;
            }

            var warning = cargo.IsMentionable
                ? string.Empty
                : "\n\n⚠️ This role **is not mentionable**, so staff won't be notified when " +
                  "a ticket opens — the mention shows up as plain text.";

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Staff role set",
                    $"{cargo.Mention} can now see every ticket in this server.{warning}")));
        }

        [SlashCommand("tickets-log", "Channel that receives the transcript when a ticket closes")]
        public Task TicketLogCommand(
            InteractionContext ctx,
            [ChannelTypes(ChannelType.Text, ChannelType.News)]
            [Option("channel", "Leave empty to stop archiving")] DiscordChannel? canal = null) =>
            ApplyAsync(ctx, canal,
                // AttachFiles porque o transcript e um anexo: sem ela o
                // fechamento falha e o canal do ticket fica de pe, e descobrir
                // isso no primeiro fechamento e tarde demais.
                needs: Permissions.SendMessages | Permissions.EmbedLinks | Permissions.AttachFiles,
                assign: (s, v) => s.ticketLogChannelId = v,
                onSet: c => Embeds.Ok("Ticket log set",
                    $"Each closed ticket's transcript goes to {c.Mention}, and only then is the channel deleted."),
                onClear: () => Embeds.Ok("Ticket log off",
                    "Without a log channel there is nowhere to archive, so closing a ticket will **lock** the " +
                    "channel instead of deleting it — the conversation is never lost without a copy."));

        [SlashCommand("verify-role", "Role given to members who verify their Roblox account")]
        public Task VerifiedRoleCommand(
            InteractionContext ctx,
            [Option("role", "Leave empty to give no role")] DiscordRole? cargo = null) =>
            ApplyRoleAsync(ctx, cargo,
                conflict: s => s.unverifiedRoleId,
                assign: (s, v) => s.verifiedRoleId = v,
                onSet: r => Embeds.Ok("Verified role set",
                    $"Anyone who links their Roblox account gets {r.Mention} — right away through `/verify`, or on joining if " +
                    "they already verified in another server. Members who were verified before get it on their next `/update`."),
                onClear: () => Embeds.Ok("Verified role off",
                    "Verifying an account no longer gives a role. Members who already have it keep it."));

        [SlashCommand("verify-unverified-role", "Role for members who haven't linked a Roblox account yet")]
        public Task UnverifiedRoleCommand(
            InteractionContext ctx,
            [Option("role", "Leave empty to turn it off")] DiscordRole? cargo = null) =>
            ApplyRoleAsync(ctx, cargo,
                conflict: s => s.verifiedRoleId,
                assign: (s, v) => s.unverifiedRoleId = v,
                // O cargo so e dado em eventos (entrada, /update, /unverify): o
                // bot nao varre o servidor. A resposta diz isso para ninguem
                // esperar ver os membros antigos com o cargo de uma hora para a outra.
                onSet: r => Embeds.Ok("Unverified role set",
                    $"Anyone who **joins** without a linked account gets {r.Mention}, and loses it on verifying. " +
                    "Members already in the server don't get it automatically — only when they run `/update`."),
                onClear: () => Embeds.Ok("Unverified role off",
                    "No one gets a role for being unverified anymore. Members who already have it keep it."));

        [SlashCommand("verify-nickname", "How the bot sets the nickname of members who verify their Roblox account")]
        public async Task NicknameCommand(
            InteractionContext ctx,
            [Choice("Don't change the nickname", NicknameFormats.None)]
            [Choice("Roblox username", NicknameFormats.Username)]
            [Choice("Roblox display name", NicknameFormats.Display)]
            [Choice("Display name (@username), like BloxLink", NicknameFormats.Smart)]
            [Option("format", "What goes in the nickname")] string formato)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            var value = formato is NicknameFormats.Username or NicknameFormats.Display or NicknameFormats.Smart
                ? formato
                : null;

            await GuildSettingsStore.Instance.SetAsync(ctx.Guild!.Id, ctx.User.Id, s => s.nicknameFormat = value);

            var warning = value is not null && ctx.Guild.CurrentMember is { } bot &&
                          (bot.Permissions & (Permissions.ManageNicknames | Permissions.Administrator)) == 0
                ? "\n\n⚠️ I'm missing **Manage Nicknames**: without it, no nickname will change."
                : string.Empty;

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(value is null
                ? Embeds.Ok("Nicknames left alone", "The bot no longer changes the nickname of members who verify.")
                : Embeds.Ok("Nickname format set",
                    $"Members who verify will now be nicknamed with their **{NicknameFormats.Describe(value)}**. The server " +
                    $"owner and anyone above my role are exempt, due to a Discord limitation.{warning}")));
        }

        [SlashCommand("verify-min-age", "Rejects Roblox accounts younger than this, in days")]
        public async Task MinimumAgeCommand(
            InteractionContext ctx,
            [Option("days", "Minimum Roblox account age, in days. 0 turns it off")] long dias)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            if (dias is < 0 or > 3650)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Invalid value", "The minimum age goes from 0 (off) to 3650 days.")));
                return;
            }

            await GuildSettingsStore.Instance.SetAsync(ctx.Guild!.Id, ctx.User.Id, s => s.minAccountAgeDays = (int)dias);

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(dias == 0
                ? Embeds.Ok("Anti-alt filter off", "Any linked Roblox account now counts in this server.")
                : Embeds.Ok("Anti-alt filter on",
                    $"Roblox accounts younger than **{dias} day(s)** can still be linked, but in this server they " +
                    "are treated as unverified: no verified role and no group roles.")));
        }

        [SlashCommand("leveling", "Turns XP and levels on or off in this server")]
        public async Task LevelingCommand(InteractionContext ctx,
            [Option("enabled", "Members earn XP for chatting and can level up")] bool ligado)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            await GuildSettingsStore.Instance.SetAsync(ctx.Guild!.Id, ctx.User.Id, s => s.levelingEnabled = ligado);

            // Desligar nao apaga nada: o XP fica guardado para quando religar, so
            // para de crescer e sai do ranking global.
            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(ligado
                ? Embeds.Ok("Levels on",
                    "Members now earn XP for chatting (once a minute at most) and get a DM when they level up — " +
                    "they can turn the DM off. Try `/rank`, `/leaderboard` and `/xp give`.")
                : Embeds.Ok("Levels off",
                    "Nobody earns XP here anymore and this server leaves the global ranking. The XP already earned is " +
                    "kept, so turning levels back on picks up where it left off.")));
        }

        [SlashCommand("economy", "Turns the SOL$ economy on or off in this server")]
        public async Task EconomyCommand(InteractionContext ctx,
            [Option("enabled", "Members can use /daily, /work, /pay and /balance here")] bool ligado)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            await GuildSettingsStore.Instance.SetAsync(ctx.Guild!.Id, ctx.User.Id, s => s.economyEnabled = ligado);

            // A carteira e global: ligar ou desligar aqui so decide se os
            // comandos funcionam NESTE servidor, nunca mexe em saldo de ninguem.
            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(ligado
                ? Embeds.Ok("Economy on",
                    "Members can now use `/daily`, `/work`, `/pay` and `/balance` here. Wallets are global: the same " +
                    "SOL$ balance follows each person to every server where the economy is on.")
                : Embeds.Ok("Economy off",
                    "The economy commands no longer work in this server. Nobody's wallet changes — balances are global " +
                    "and stay as they are.")));
        }

        /// <summary>
        /// O caminho comum dos subcomandos de canal: confere que o canal e deste
        /// servidor, confere as permissoes que aquele recurso vai precisar, grava,
        /// e responde.
        ///
        /// A conferencia de servidor parece redundante - o seletor do Discord so
        /// oferece canais daqui -, mas e barata e a alternativa e confiar num
        /// dado que veio de fora. A de permissao e o ponto: sem ela o operador
        /// so descobre o problema quando o recurso falha calado, dias depois.
        /// </summary>
        private static async Task ApplyAsync(
            InteractionContext ctx,
            DiscordChannel? channel,
            Permissions needs,
            Action<GuildSettings, ulong?> assign,
            Func<DiscordChannel, DiscordEmbed> onSet,
            Func<DiscordEmbed> onClear)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            if (channel is not null)
            {
                if (channel.GuildId != ctx.Guild!.Id)
                {
                    await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                        Embeds.Error("Channel from another server",
                            "Only channels from this server can be configured.")));
                    return;
                }

                if (needs != Permissions.None && ctx.Guild.CurrentMember is { } bot)
                {
                    var missing = needs & ~channel.PermissionsFor(bot);
                    if (missing != Permissions.None)
                    {
                        await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                            Embeds.Error("Missing permissions in that channel",
                                $"I can't use {channel.Mention} because I'm missing: **{missing}**.\n\n" +
                                "Adjust the channel's permissions and run the command again.")));
                        return;
                    }
                }
            }

            await GuildSettingsStore.Instance.SetAsync(ctx.Guild!.Id, ctx.User.Id,
                s => assign(s, channel?.Id));

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                channel is null ? onClear() : onSet(channel)));
        }

        /// <summary>
        /// O caminho comum dos subcomandos de cargo da verificacao. O cargo passa
        /// pela mesma checagem do /bind: o bot vai distribui-lo sozinho, entao ele
        /// nao pode estar acima de quem configura nem acima do bot.
        /// </summary>
        private static async Task ApplyRoleAsync(
            InteractionContext ctx,
            DiscordRole? role,
            Func<GuildSettings, ulong?> conflict,
            Action<GuildSettings, ulong?> assign,
            Func<DiscordRole, DiscordEmbed> onSet,
            Func<DiscordEmbed> onClear)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            if (role is not null)
            {
                if (VerificationService.CheckAssignableRole(ctx.Guild!, ctx.Member!, role) is { } refusal)
                {
                    await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(refusal));
                    return;
                }

                // Verificado e nao verificado no mesmo cargo: o bot daria e tiraria
                // o mesmo cargo a cada evento.
                if (GuildSettingsStore.For(ctx.Guild!.Id) is { } current && conflict(current) == role.Id)
                {
                    await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(Embeds.Error("Same role",
                        $"{role.Mention} is already the other verification role. Verified and unverified must be different roles.")));
                    return;
                }
            }

            await GuildSettingsStore.Instance.SetAsync(ctx.Guild!.Id, ctx.User.Id, s => assign(s, role?.Id));

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                role is null ? onClear() : onSet(role)));
        }

        private static string DescribeRole(DiscordGuild guild, ulong? roleId, string command)
        {
            if (roleId is not { } id)
                return $"— *({command})*";

            return guild.GetRole(id) is { } role
                ? role.Mention
                : $"⚠️ role `{id}` no longer exists";
        }

        private static string Describe(DiscordGuild guild, ulong? channelId, string command)
        {
            if (channelId is not { } id)
                return $"— *({command})*";

            return guild.GetChannel(id) is { } channel
                ? channel.Mention
                : $"⚠️ channel `{id}` no longer exists";
        }

        /// <summary>
        /// O que ainda falta para cada recurso funcionar. E a pergunta que quem
        /// configura de fato tem, e que uma lista de valores nao responde.
        /// </summary>
        private static List<string> Pending(DiscordGuild guild, GuildSettings? settings, bool verifyReady)
        {
            var pending = new List<string>();

            var verification = VerificationService.IsConfigured(settings);
            if (verification && !verifyReady)
                pending.Add("Roblox verification is set up here, but **whoever runs the bot** hasn't turned it on yet: " +
                            "`/verify` replies that it's unavailable");

            var hasCategory = settings?.ticketCategoryId is > 0;
            var hasRole = settings?.ticketStaffRoleId is > 0;

            if (hasCategory ^ hasRole)
                pending.Add("Tickets need **both a category and a role** — just one of them doesn't turn anything on");

            if (hasCategory && hasRole && settings?.ticketLogChannelId is not > 0)
                pending.Add("Without a ticket log channel, closing a ticket **locks** the channel instead of deleting it");

            if (settings?.autoSoftbanChannelId is > 0 && settings?.moderationLogChannelId is not > 0)
                pending.Add("With the trap armed and no moderation log, the notice that the circuit breaker disarmed it " +
                            "doesn't reach any channel");

            if (guild.CurrentMember is { } bot)
            {
                if (hasCategory && (bot.Permissions & (Permissions.ManageChannels | Permissions.ManageRoles)) !=
                    (Permissions.ManageChannels | Permissions.ManageRoles))
                    pending.Add("I'm missing **Manage Channels** and/or **Manage Roles**, and without them I can't create " +
                                "a ticket channel");

                if (settings?.autoSoftbanChannelId is > 0 && (bot.Permissions & Permissions.BanMembers) == 0)
                    pending.Add("I'm missing **Ban Members**, and without it every auto-softban will fail");

                var usesRoles = settings is not null &&
                                (settings.verifiedRoleId is > 0 || settings.unverifiedRoleId is > 0 || settings.groupBinds.Count > 0);
                if (usesRoles && (bot.Permissions & (Permissions.ManageRoles | Permissions.Administrator)) == 0)
                    pending.Add("I'm missing **Manage Roles**, and without it verification can't give or remove any role");

                if (settings?.nicknameFormat is not (null or NicknameFormats.None) &&
                    (bot.Permissions & (Permissions.ManageNicknames | Permissions.Administrator)) == 0)
                    pending.Add("I'm missing **Manage Nicknames**, and without it the nickname of members who verify won't change");
            }

            return pending;
        }
    }
}
