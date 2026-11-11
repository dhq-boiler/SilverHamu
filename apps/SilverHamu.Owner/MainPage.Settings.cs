using SilverHamu.Owner.Services;
using SilverHamu.Owner.Controls;

namespace SilverHamu.Owner;

public sealed partial class MainPage
{
    private const string LanguagePreference = "Owner.Language";
    private readonly Grid settingsOverlay = new() { IsVisible = false, BackgroundColor = Color.FromArgb("#E010171E"), AutomationId = "SettingsDialog" };
    private readonly RadioButton japaneseLanguage = new() { Content = "日本語", GroupName = "Language", AutomationId = "LanguageJapanese", TextColor = Colors.White };
    private readonly RadioButton englishLanguage = new() { Content = "English", GroupName = "Language", AutomationId = "LanguageEnglish", TextColor = Colors.White };

    private void BuildSettingsDialog()
    {
        var panel = new VerticalStackLayout { Padding = 28, Spacing = 16, WidthRequest = 420, BackgroundColor = Surface };
        panel.Add(LocalizedText("Settings", 24, true));
        panel.Add(LocalizedText("DisplayLanguage", 14, true));
        panel.Add(LocalizedText("LanguageHelp", 12, color: Muted));
        japaneseLanguage.IsChecked = L.Instance.Language == "ja";
        englishLanguage.IsChecked = L.Instance.Language == "en";
        panel.Add(japaneseLanguage);
        panel.Add(englishLanguage);
        japaneseLanguage.CheckedChanged += (_, args) => { if (args.Value) ChangeLanguage("ja"); };
        englishLanguage.CheckedChanged += (_, args) => { if (args.Value) ChangeLanguage("en"); };
        var close = new Button { AutomationId = "SettingsClose", BackgroundColor = Accent, TextColor = CanvasColor }.WithActionIcon();
        LocalizationBindings.Bind(close, Button.TextProperty, "Close");
        close.Clicked += (_, _) => settingsOverlay.IsVisible = false;
        panel.Add(close);
        settingsOverlay.Add(new Border { Content = panel, HorizontalOptions = LayoutOptions.Center, VerticalOptions = LayoutOptions.Center });
    }

    private void ChangeLanguage(string language)
    {
        Preferences.Default.Set(LanguagePreference, language);
        completionController?.Hide();
        L.SetLanguage(language);
        RefreshLanguage();
    }

    private void RefreshLanguage()
    {
        ConfigureQueryLayout();
        filter.Placeholder = L.Get("FilterDisplayedResults");
        if (token.Handler?.PlatformView is Microsoft.UI.Xaml.Controls.PasswordBox nativeToken)
            nativeToken.PlaceholderText = L.Get("TokenNotSaved");
        transaction.Text = L.Get(inTransaction ? "UncommittedTransaction" : "NoTransaction");
        AttachCompletion();
        RenderTabs(); RenderExplorer(); RenderRows(); UpdateMode();
        if (loadedResult is not null)
        {
            currentPage.Text = $"{resultPage + 1:N0}";
            var start = loadedResult.RowCount == 0 ? 0 : resultPage * LoadedQueryResult.PageSize + 1;
            pagePosition.Text = L.Get("ResultPagePosition", loadedResult.PageCount, start, resultPage * LoadedQueryResult.PageSize + result.Rows.Count, loadedResult.RowCount);
            SemanticProperties.SetDescription(currentPage, L.Get("CurrentPageNOpenPageSelection", resultPage + 1));
        }
    }

    private static Label LocalizedText(string key, double size, bool bold = false, Color? color = null)
    {
        var label = Text("", size, bold, color);
        LocalizationBindings.Bind(label, Label.TextProperty, key);
        return label;
    }

    private Button LocalizedAction(string key, string id, Func<Task> action, bool primary = false)
    {
        var button = Action("", id, action, primary);
        LocalizationBindings.Bind(button, Button.TextProperty, key);
        LocalizationBindings.Bind(button, SemanticProperties.DescriptionProperty, key);
        return button;
    }
}
