using System.ComponentModel;
using System.Globalization;
using System.Resources;

namespace SilverHamu.Owner.Services;

internal sealed class L : INotifyPropertyChanged
{
    private static readonly ResourceManager Resources = new("SilverHamu.Owner.Resources.Localization.Strings", typeof(L).Assembly);
    public static L Instance { get; } = new();
    public string Language { get; private set; } = "ja";
    public event PropertyChangedEventHandler? PropertyChanged;

    public static string ResolveLanguage(string? saved, string systemLanguage) =>
        saved is "ja" or "en" ? saved : systemLanguage.StartsWith("ja", StringComparison.OrdinalIgnoreCase) ? "ja" : "en";

    public static void SetLanguage(string language)
    {
        if (language is not ("ja" or "en")) throw new ArgumentOutOfRangeException(nameof(language));
        if (Instance.Language == language) return;
        Instance.Language = language;
        Instance.PropertyChanged?.Invoke(Instance, new PropertyChangedEventArgs(nameof(Language)));
    }

    public static string Get(string key, params object?[] arguments) => GetForLanguage(Instance.Language, key, arguments);

    public static string GetForLanguage(string language, string key, params object?[] arguments)
    {
        var culture = CultureInfo.GetCultureInfo(language);
        var format = Resources.GetString(key, culture) ?? throw new MissingManifestResourceException(key);
        return string.Format(culture, format, arguments);
    }
}
