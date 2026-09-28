using System;
using System.Threading.Tasks;
using CommunityBot.Services;
using CommunityBot.Services.Roblox;
using DisCatSharp.ApplicationCommands;
using DisCatSharp.ApplicationCommands.Attributes;
using DisCatSharp.ApplicationCommands.Context;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.Enums.Core;

namespace CommunityBot.commands
{
    /// <summary>
    /// Verificacao de conta Roblox, no molde do BloxLink.
    ///
    /// Os nomes (/verify, /unverify, /update, /whois) sao os do BloxLink de
    /// proposito: e o que quem vem de outro servidor ja sabe digitar.
    ///
    /// A logica mora em Services/Roblox. Aqui fica so a porta de entrada, porque
    /// o botao do painel precisa fazer exatamente o mesmo que o /verify.
    /// </summary>
    [ApplicationCommandRequireGuild]
    internal class Verification : ApplicationCommandsModule
    {
        [SlashCommand("verify", "Vincula a sua conta Roblox pelo login oficial do Roblox")]
        [SlashCommandCooldown(3, 60, CooldownBucketType.User)]
        public Task VerifyCommand(InteractionContext ctx) =>
            VerificationFlow.BeginAsync(ctx.Client, ctx.Interaction, ctx.Guild!, ctx.Member!);

        [SlashCommand("unverify", "Desfaz o vínculo da sua conta Roblox, em todos os servidores")]
        public async Task UnverifyCommand(InteractionContext ctx)
        {
            if (RobloxLinkStore.For(ctx.User.Id) is not { } link)
            {
                await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                    new DiscordInteractionResponseBuilder().AsEphemeral().AddEmbed(Embeds.Info("Nada a desfazer",
                        "Sua conta do Discord não está vinculada a nenhuma conta Roblox.")));
                return;
            }

            // Confirmacao por botao: o vinculo e global, e desfaze-lo tira cargo
            // de verificacao em todo servidor na proxima atualizacao.
            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder()
                    .AsEphemeral()
                    .AddEmbed(new DiscordEmbedBuilder()
                        .WithTitle("Desvincular a conta Roblox?")
                        .WithColor(DiscordColor.Blurple)
                        .WithDescription(
                            $"Hoje você está vinculado a {VerificationFlow.Describe(link)}.\n\n" +
                            "Desvincular vale para **todos os servidores** que usam o Sollarety. Aqui os cargos de " +
                            "verificação saem na hora; nos outros, no próximo `/update`. Para voltar, é só `/verify`.")
                        .Build())
                    .AddComponents(new DiscordButtonComponent(ButtonStyle.Danger, VerificationFlow.UnlinkId, "Desvincular")));
        }

        [SlashCommand("update", "Relê a conta Roblox e reaplica cargos e apelido")]
        [SlashCommandCooldown(5, 60, CooldownBucketType.User)]
        public async Task UpdateCommand(
            InteractionContext ctx,
            [Option("usuario", "Quem atualizar. Vazio atualiza você; outra pessoa exige Gerenciar Cargos")] DiscordUser? usuario = null)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            var target = usuario ?? ctx.User;
            var self = target.Id == ctx.User.Id;

            // Atualizar outra pessoa so reaplica o estado dela - nao da cargo que
            // ela nao mereca. Mesmo assim mexe em cargo alheio, entao pede a
            // permissao de quem mexe em cargo.
            if (!self && (ctx.Member!.Permissions & (Permissions.ManageRoles | Permissions.Administrator)) == 0)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(Embeds.Error("Sem permissão",
                    "Atualizar outra pessoa exige **Gerenciar Cargos**. Sem a opção `usuario`, você se atualiza.")));
                return;
            }

            if (target.IsBot)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Alvo inválido", "Bots não têm conta Roblox.")));
                return;
            }

            if (!VerificationService.IsConfigured(GuildSettingsStore.For(ctx.Guild!.Id)))
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(Embeds.Info("Nada configurado",
                    "Este servidor ainda não configurou a verificação Roblox, então não há cargo a aplicar. " +
                    "Quem tem Gerenciar Servidor configura com `/config verificacao-cargo` e `/bind adicionar`.")));
                return;
            }

            var member = await Hierarchy.TryGetMemberAsync(ctx, target.Id);
            if (member is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Fora do servidor", $"{target.Mention} não está neste servidor.")));
                return;
            }

            var link = RobloxLinkStore.For(target.Id);
            if (link is not null)
            {
                // Relido do zero: o /update existe justamente para pegar troca de
                // nome ou promocao no grupo que o cache de cinco minutos esconderia.
                RobloxApi.Forget(link.robloxId);
                var user = await RobloxApi.GetUserAsync(link.robloxId, fresh: true);
                if (user.Ok)
                    link = await RobloxLinkStore.Instance.RefreshAsync(target.Id, user.Value!) ?? link;
            }

            var report = await VerificationService.ApplyAsync(ctx.Client, ctx.Guild!, member, link,
                AuditReason.For(ctx.User.UsernameWithDiscriminator, "/update da verificação Roblox"), fresh: true);

            var embed = new DiscordEmbedBuilder()
                .WithTitle(self ? "Você foi atualizado" : "Membro atualizado")
                .WithColor(DiscordColor.SpringGreen)
                .WithDescription(link is null
                    ? $"{target.Mention} não tem conta Roblox vinculada." + (self ? " Use `/verify` para vincular." : string.Empty)
                    : $"{target.Mention} → {VerificationFlow.Describe(link)}");

            if (report.Refusal is not null)
                embed.AddField(new DiscordEmbedField("Não liberado neste servidor",
                    Embeds.Trim("A conta está vinculada, mas " + report.Refusal, 1024), false));

            embed.AddField(new DiscordEmbedField("Mudanças", report.Describe(), false));

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(embed));
        }

        [SlashCommand("whois", "Mostra a conta Roblox vinculada a alguém")]
        public async Task WhoisCommand(
            InteractionContext ctx,
            [Option("usuario", "Quem consultar")] DiscordUser usuario)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            // So membros DESTE servidor. A opcao de usuario aceita qualquer id
            // colado, e sem isto o /whois viraria uma busca do vinculo Roblox de
            // qualquer pessoa do Discord - a politica de privacidade promete que
            // so quem divide um servidor com ela ve.
            if (await Hierarchy.TryGetMemberAsync(ctx, usuario.Id) is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Fora do servidor", $"{usuario.Mention} não está neste servidor.")));
                return;
            }

            DiscordEmbed embed;

            if (RobloxLinkStore.For(usuario.Id) is not { } link)
            {
                embed = Embeds.Info("Sem vínculo", $"{usuario.Mention} não tem conta Roblox vinculada.");
            }
            else
            {
                var builder = new DiscordEmbedBuilder()
                    .WithTitle("Conta Roblox vinculada")
                    .WithColor(DiscordColor.Blurple)
                    .WithDescription($"{usuario.Mention} → {VerificationFlow.Describe(link)}")
                    .AddField(new DiscordEmbedField("ID Roblox", $"`{link.robloxId}`", true))
                    .AddField(new DiscordEmbedField("Vinculada em", $"<t:{link.verifiedAtUtc.ToUnixTimeSeconds()}:D>", true));

                if (link.robloxCreatedUtc is { } created)
                    builder.AddField(new DiscordEmbedField("Conta criada em", $"<t:{created.ToUnixTimeSeconds()}:D>", true));

                embed = builder.Build();
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(embed));
        }

        [SlashCommand("verify-painel", "Publica neste canal o painel com o botão de verificação",
            (long)Permissions.ManageGuild)]
        [ApplicationCommandRequireUserPermissions(Permissions.ManageGuild)]
        [ApplicationCommandRequireBotPermissions(Permissions.SendMessages)]
        public async Task PanelCommand(
            InteractionContext ctx,
            [Option("titulo", "Título do painel")] string? titulo = null,
            [Option("texto", "Texto explicativo do painel")] string? texto = null)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            var embed = new DiscordEmbedBuilder()
                .WithTitle(Embeds.SafeTrim(titulo ?? "Verifique sua conta Roblox", 200))
                .WithDescription(Embeds.SafeTrim(texto ??
                    "Clique no botão abaixo para vincular sua conta Roblox pelo login oficial do Roblox. " +
                    "Quem já verificou em outro servidor com o Sollarety recebe os cargos na hora.", 2000))
                .WithColor(DiscordColor.Blurple);

            // O botao nao carrega nada alem da acao: quem clicou e o que importa,
            // e isso vem do proprio clique. O painel sobrevive a reinicios.
            var message = new DiscordMessageBuilder()
                .AddEmbed(embed)
                .AddComponents(new DiscordButtonComponent(ButtonStyle.Success, VerificationFlow.StartId, "Verificar",
                    emoji: new DiscordComponentEmoji("✅")));

            try
            {
                await ctx.Channel.SendMessageAsync(message);
            }
            catch (Exception ex)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Falha ao publicar o painel", Embeds.Trim(ex.Message, 500))));
                return;
            }

            var note = VerificationService.IsConfigured(GuildSettingsStore.For(ctx.Guild!.Id))
                ? string.Empty
                : "\n\n⚠️ Este servidor ainda não configurou **nenhum cargo** de verificação: o botão vincula a conta, " +
                  "mas não dá nada. Veja `/config verificacao-cargo` e `/bind adicionar`.";

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Painel publicado", $"O painel está de pé em {ctx.Channel.Mention}.{note}")));
        }
    }
}
