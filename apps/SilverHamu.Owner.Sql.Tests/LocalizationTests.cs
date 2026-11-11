using System.Collections;
using System.Globalization;
using System.Resources;
using System.Text;
using SilverHamu.Owner.Services;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class LocalizationTests
{
    [Theory]
    [InlineData("ja", "en-US", "ja")]
    [InlineData("en", "ja-JP", "en")]
    [InlineData(null, "ja-JP", "ja")]
    [InlineData("", "ja", "ja")]
    [InlineData(null, "en-US", "en")]
    [InlineData(null, "fr-FR", "en")]
    [InlineData("fr", "ja-JP", "ja")]
    [InlineData("invalid", "", "en")]
    public void Saved_choice_takes_priority_and_other_system_languages_fall_back_to_English(string? saved, string system, string expected) =>
        Assert.Equal(expected, L.ResolveLanguage(saved, system));

    [Theory]
    [InlineData("ja")]
    [InlineData("en")]
    public void Both_languages_have_every_resource_and_matching_format_arguments(string language)
    {
        var manager = new ResourceManager("SilverHamu.Owner.Resources.Localization.Strings", typeof(L).Assembly);
        var english = manager.GetResourceSet(CultureInfo.InvariantCulture, true, false)!;
        var localized = manager.GetResourceSet(CultureInfo.GetCultureInfo(language == "ja" ? "ja" : ""), true, false)!;
        var expectedKeys = english.Cast<DictionaryEntry>().Select(e => (string)e.Key).Order().ToArray();
        Assert.Equal(expectedKeys, localized.Cast<DictionaryEntry>().Select(e => (string)e.Key).Order().ToArray());
        foreach (var key in expectedKeys)
        {
            var format = (string)localized.GetString(key)!;
            Assert.False(string.IsNullOrWhiteSpace(format), key);
            var expected = CompositeFormat.Parse(english.GetString(key)!);
            var actual = CompositeFormat.Parse(format);
            Assert.Equal(expected.MinimumArgumentCount, actual.MinimumArgumentCount);
            var values = Enumerable.Repeat<object>(1234, actual.MinimumArgumentCount).ToArray();
            Assert.Equal(string.Format(CultureInfo.GetCultureInfo(language), format, values), L.GetForLanguage(language, key, values));
        }
    }

    [Theory]
    [InlineData("en", "Settings")]
    [InlineData("ja", "設定")]
    public void Settings_title_uses_the_requested_language(string language, string expected) =>
        Assert.Equal(expected, L.GetForLanguage(language, "Settings"));

    [Fact]
    public void Localized_SQL_templates_keep_the_uneditable_marker_syntax()
    {
        Assert.Equal("{{value_3}}", L.GetForLanguage("en", "TemplateValue", 3));
        Assert.Equal("{{値_3}}", L.GetForLanguage("ja", "TemplateValue", 3));
    }
}
