using Microsoft.UI.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Windows.System;
using Windows.UI.Core;
using NativeBorder = Microsoft.UI.Xaml.Controls.Border;
using NativeTextBox = Microsoft.UI.Xaml.Controls.TextBox;
using NativeListView = Microsoft.UI.Xaml.Controls.ListView;
using NativeTextBlock = Microsoft.UI.Xaml.Controls.TextBlock;
using AutomationProperties = Microsoft.UI.Xaml.Automation.AutomationProperties;
using SolidColorBrush = Microsoft.UI.Xaml.Media.SolidColorBrush;

namespace SilverHamu.Owner.Controls;

internal sealed class SqlCompletionController : IDisposable
{
    private readonly NativeTextBox editor;
    private readonly Func<IEnumerable<SqlTable>> schema;
    private readonly NativeListView list = new() { MaxHeight = 220, IsItemClickEnabled = true, IsTabStop = false };
    private readonly Popup popup = new() { IsLightDismissEnabled = false };
    private SqlCompletionSet? suggestions;
    private string suggestedText = "";
    private bool updating, composing, disposed;

    public SqlCompletionController(NativeTextBox editor, Func<IEnumerable<SqlTable>> schema)
    {
        this.editor = editor; this.schema = schema;
        AutomationProperties.SetAutomationId(list, "SqlCompletionList");
        AutomationProperties.SetName(list, L.Get("SQLSuggestions"));
        var panel = new Microsoft.UI.Xaml.Controls.StackPanel();
        panel.Children.Add(new NativeTextBlock
        {
            Text = L.Get("SqlCompletionHelp"),
            FontSize = 12, Margin = new Microsoft.UI.Xaml.Thickness(10, 8, 10, 4)
        });
        panel.Children.Add(list);
        popup.Child = new NativeBorder
        {
            Child = panel, Width = 440, BorderThickness = new Microsoft.UI.Xaml.Thickness(1),
            Background = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 24, 35, 45)),
            BorderBrush = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 89, 217, 179)),
            RequestedTheme = ElementTheme.Dark
        };
        list.ItemClick += OnItemClick;
        editor.TextChanged += OnTextChanged;
        editor.SelectionChanged += OnSelectionChanged;
        editor.PreviewKeyDown += OnKeyDown;
        editor.LostFocus += OnLostFocus;
        editor.TextCompositionStarted += OnCompositionStarted;
        editor.TextCompositionEnded += OnCompositionEnded;
        editor.SizeChanged += OnSizeChanged;
    }

    public void Show() { editor.Focus(FocusState.Programmatic); Update(force: true); }
    public void Hide() => popup.IsOpen = false;

    private void Update(bool force = false)
    {
        if (disposed || updating || composing || editor.XamlRoot is null || editor.SelectionLength != 0 ||
            editor.FocusState == FocusState.Unfocused) { Hide(); return; }
        suggestions = SqlCompletion.Suggest(editor.Text, editor.SelectionStart, schema(), force);
        suggestedText = editor.Text;
        if (suggestions.Items.Count == 0) { Hide(); return; }
        list.Items.Clear();
        for (int i = 0; i < suggestions.Items.Count; i++)
        {
            var item = suggestions.Items[i];
            var button = new Microsoft.UI.Xaml.Controls.Button
            {
                Content = item.Label, HorizontalAlignment = Microsoft.UI.Xaml.HorizontalAlignment.Stretch,
                HorizontalContentAlignment = Microsoft.UI.Xaml.HorizontalAlignment.Left,
                FontFamily = new("Consolas"), FontSize = 14, IsTabStop = false,
                Background = new SolidColorBrush(Windows.UI.Color.FromArgb(0, 0, 0, 0)),
                BorderThickness = new Microsoft.UI.Xaml.Thickness(0)
            };
            button.Click += (_, _) => Accept(item);
            AutomationProperties.SetAutomationId(button, $"SqlCompletion{i}");
            AutomationProperties.SetName(button, item.Label);
            var row = new ListViewItem { Content = button, Tag = item, HorizontalContentAlignment = Microsoft.UI.Xaml.HorizontalAlignment.Stretch };
            list.Items.Add(row);
        }
        list.SelectedIndex = 0;
        popup.XamlRoot = editor.XamlRoot;
        var position = editor.TransformToVisual(null).TransformPoint(new Windows.Foundation.Point(0, editor.ActualHeight));
        popup.HorizontalOffset = Math.Max(0, Math.Min(position.X, editor.XamlRoot.Size.Width - 440));
        popup.VerticalOffset = Math.Max(0, Math.Min(position.Y, editor.XamlRoot.Size.Height - 270));
        popup.IsOpen = true;
    }

    private void Accept(SqlCompletionItem item)
    {
        if (suggestions is null || editor.Text != suggestedText || composing) { Hide(); return; }
        updating = true;
        try
        {
            var replacement = suggestions.Apply(editor.Text, item);
            editor.Text = replacement.Text;
            editor.SelectionStart = replacement.Cursor;
            editor.SelectionLength = 0;
            Hide();
            editor.Focus(FocusState.Programmatic);
        }
        finally { updating = false; }
    }

    private void OnKeyDown(object sender, KeyRoutedEventArgs args)
    {
        if (composing) return;
        bool control = (InputKeyboardSource.GetKeyStateForCurrentThread(VirtualKey.Control) & CoreVirtualKeyStates.Down) != 0;
        bool shift = (InputKeyboardSource.GetKeyStateForCurrentThread(VirtualKey.Shift) & CoreVirtualKeyStates.Down) != 0;
        if (control && args.Key == VirtualKey.Space) { Update(force: true); args.Handled = true; return; }
        if (!popup.IsOpen) return;
        if (args.Key == VirtualKey.Escape) { Hide(); args.Handled = true; }
        else if (!control && !shift && args.Key is VirtualKey.Down or VirtualKey.Up)
        {
            list.SelectedIndex = Math.Clamp(list.SelectedIndex + (args.Key == VirtualKey.Down ? 1 : -1), 0, list.Items.Count - 1);
            list.ScrollIntoView(list.SelectedItem); args.Handled = true;
        }
        else if (!control && !shift && args.Key is VirtualKey.Enter or VirtualKey.Tab && list.SelectedItem is ListViewItem { Tag: SqlCompletionItem item })
        { Accept(item); args.Handled = true; }
        else if (args.Key == VirtualKey.Tab) Hide();
    }

    private void OnTextChanged(object sender, Microsoft.UI.Xaml.Controls.TextChangedEventArgs args) => Update();
    private void OnSelectionChanged(object sender, RoutedEventArgs args) => Update();
    private void OnItemClick(object sender, ItemClickEventArgs args)
    { if (args.ClickedItem is ListViewItem { Tag: SqlCompletionItem item }) Accept(item); }
    private void OnCompositionStarted(NativeTextBox sender, TextCompositionStartedEventArgs args)
    { composing = true; Hide(); }
    private void OnCompositionEnded(NativeTextBox sender, TextCompositionEndedEventArgs args)
    { composing = false; editor.DispatcherQueue.TryEnqueue(() => Update()); }
    private void OnSizeChanged(object sender, SizeChangedEventArgs args) => Hide();
    private void OnLostFocus(object sender, RoutedEventArgs args) => editor.DispatcherQueue.TryEnqueue(() =>
    {
        if (disposed || editor.XamlRoot is null) return;
        var focused = FocusManager.GetFocusedElement(editor.XamlRoot) as DependencyObject;
        while (focused is not null)
        {
            if (focused == list || focused == editor) return;
            focused = VisualTreeHelper.GetParent(focused);
        }
        Hide();
    });

    public void Dispose()
    {
        if (disposed) return;
        disposed = true;
        Hide();
        list.ItemClick -= OnItemClick;
        editor.TextChanged -= OnTextChanged;
        editor.SelectionChanged -= OnSelectionChanged;
        editor.PreviewKeyDown -= OnKeyDown;
        editor.LostFocus -= OnLostFocus;
        editor.TextCompositionStarted -= OnCompositionStarted;
        editor.TextCompositionEnded -= OnCompositionEnded;
        editor.SizeChanged -= OnSizeChanged;
    }
}
