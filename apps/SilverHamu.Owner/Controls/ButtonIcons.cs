using Symbol = Microsoft.UI.Xaml.Controls.Symbol;

namespace SilverHamu.Owner.Controls;

internal static class ButtonIcons
{
    public static Button WithActionIcon(this Button button)
    {
        Symbol? symbol = button.AutomationId switch
        {
            "Settings" => Symbol.Setting,
            "Connect" or "ConnectSubmit" => Symbol.Globe,
            "OpenEmbedded" or "OpenQuery" or "TokenFile" or "FileBrowse" => Symbol.OpenFile,
            "CreateEmbedded" or "Begin" => Symbol.Add,
            "BackupEmbedded" => Symbol.Copy,
            "RestoreEmbedded" => Symbol.Upload,
            "RawKeyValue" => Symbol.Edit,
            "ExportSnapshot" or "Export" => Symbol.Download,
            "Disconnect" or "ConnectCancel" or "FileCancel" or "CsvCancel" or "SettingsClose" => Symbol.Cancel,
            "SaveQuery" or "Checkpoint" or "CsvConfirm" => Symbol.Save,
            "Execute" => Symbol.Play,
            "ReloadEditResults" => Symbol.Refresh,
            "CompleteSql" => Symbol.List,
            "Browse" => Symbol.ViewAll,
            "Commit" or "FileSubmit" => Symbol.Accept,
            "Rollback" or "UndoEdit" => Symbol.Undo,
            "RedoEdit" => Symbol.Redo,
            "ResultPrevious" => Symbol.Back,
            "ResultNext" => Symbol.Forward,
            "ResultLast" => Symbol.Next,
            "RefreshTables" => Symbol.Refresh,
            { } id when id.StartsWith("QueryTab", StringComparison.Ordinal) => Symbol.Document,
            _ => null
        };
        return symbol is { } icon ? button.WithIcon(icon) : button;
    }

    public static Button WithIcon(this Button button, Symbol symbol)
    {
        Microsoft.UI.Xaml.Controls.TextBlock? text = null;
        void Configure()
        {
            if (button.Handler?.PlatformView is not Microsoft.UI.Xaml.Controls.Button native) return;
            // MAUIのFontImageSourceはWindowsのシステムフォントを相対URIとして扱い、画像生成に失敗する。
            text = SetNativeContent(native, symbol, button.Text ?? "", button.FontSize);
        }
        button.HandlerChanged += (_, _) => Configure();
        button.PropertyChanged += (_, args) =>
        {
            if (text is null) return;
            if (args.PropertyName == nameof(Button.Text))
            {
                text.Text = button.Text;
                if (button.Handler?.PlatformView is Microsoft.UI.Xaml.Controls.Button native)
                    Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(native, button.Text ?? "");
            }
            if (args.PropertyName == nameof(Button.FontSize)) text.FontSize = button.FontSize;
        };
        Configure();
        return button;
    }

    public static Symbol QueryOperation(string operation) => operation switch
    {
        "Select" or "SelectAll" => Symbol.Find,
        "SelectEdit" or "SelectAllEdit" => Symbol.Edit,
        "Insert" => Symbol.Add,
        "Update" => Symbol.Edit,
        "Delete" => Symbol.Delete,
        _ => Symbol.Document
    };

    public static void AddNativeIcon(Microsoft.UI.Xaml.Controls.Button button, Symbol symbol)
    {
        var caption = button.Content as string ?? "";
        SetNativeContent(button, symbol, caption, button.FontSize);
    }

    private static Microsoft.UI.Xaml.Controls.TextBlock SetNativeContent(Microsoft.UI.Xaml.Controls.Button button, Symbol symbol, string caption, double fontSize)
    {
        var content = new Microsoft.UI.Xaml.Controls.StackPanel { Orientation = Microsoft.UI.Xaml.Controls.Orientation.Horizontal, Spacing = 8 };
        var icon = new Microsoft.UI.Xaml.Controls.FontIcon
        {
            FontFamily = new Microsoft.UI.Xaml.Media.FontFamily("Segoe MDL2 Assets"), Glyph = char.ConvertFromUtf32((int)symbol),
            FontSize = 14, Width = 16, VerticalAlignment = Microsoft.UI.Xaml.VerticalAlignment.Center, IsHitTestVisible = false,
            IsTextScaleFactorEnabled = false
        };
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetAccessibilityView(icon, Microsoft.UI.Xaml.Automation.Peers.AccessibilityView.Raw);
        content.Children.Add(icon);
        var text = new Microsoft.UI.Xaml.Controls.TextBlock { Text = caption, FontSize = fontSize, VerticalAlignment = Microsoft.UI.Xaml.VerticalAlignment.Center };
        content.Children.Add(text);
        button.Content = content;
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(button, caption);
        return text;
    }
}
