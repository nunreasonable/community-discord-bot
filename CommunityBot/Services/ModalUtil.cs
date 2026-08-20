using System.Collections.Generic;
using System.Linq;
using DisCatSharp.Entities;

namespace CommunityBot.Services
{
    internal static class ModalUtil
    {
        /// <summary>
        /// Le o valor de um campo de texto de um modal submetido.
        ///
        /// O Discord mudou o aninhamento dos componentes (Components V2), entao a
        /// busca e recursiva via GetChildren() em vez de assumir action rows -
        /// assim continua funcionando se o aninhamento mudar de novo.
        /// </summary>
        public static string? ReadModalValue(DiscordInteraction interaction, string customId)
        {
            var components = interaction.Data?.ModalComponents;
            if (components is null)
                return null;

            return Flatten(components)
                .OfType<DiscordTextInputComponent>()
                .FirstOrDefault(t => t.CustomId == customId)?
                .Value;
        }

        /// <summary>
        /// Le a opcao escolhida num radio group ou select de um modal submetido.
        ///
        /// Irmao do <see cref="ReadModalValue" />: la o valor mora em
        /// DiscordTextInputComponent.Value, aqui em RadioGroup.SelectedValue (ou
        /// em SelectedValues, quando o campo e um select). A travessia e a mesma,
        /// por isso reaproveita o Flatten em vez de repeti-la.
        ///
        /// Devolve null quando o campo nao foi respondido - o que so acontece se
        /// o componente tiver sido montado como opcional.
        /// </summary>
        public static string? ReadModalSelection(DiscordInteraction interaction, string customId)
        {
            var components = interaction.Data?.ModalComponents;
            if (components is null)
                return null;

            var flat = Flatten(components).ToList();

            var radio = flat
                .OfType<DiscordRadioGroupComponent>()
                .FirstOrDefault(r => r.CustomId == customId);

            if (radio is not null)
                return string.IsNullOrEmpty(radio.SelectedValue) ? null : radio.SelectedValue;

            return flat
                .OfType<DiscordBaseSelectComponent>()
                .FirstOrDefault(s => s.CustomId == customId)?
                .SelectedValues?
                .FirstOrDefault();
        }

        private static IEnumerable<DiscordComponent> Flatten(IEnumerable<DiscordComponent> components)
        {
            foreach (var component in components)
            {
                if (component is null)
                    continue;

                yield return component;

                var children = component.GetChildren();
                if (children is null)
                    continue;

                foreach (var child in Flatten(children))
                    yield return child;
            }
        }
    }
}
