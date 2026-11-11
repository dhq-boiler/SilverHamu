using System.Globalization;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Markup;
using SilverHamu.Owner.Services;
using Windows.System;
using NativeButton = Microsoft.UI.Xaml.Controls.Button;
using NativeGrid = Microsoft.UI.Xaml.Controls.Grid;
using NativeTextBox = Microsoft.UI.Xaml.Controls.TextBox;
using NativeTextBlock = Microsoft.UI.Xaml.Controls.TextBlock;
using NativeBrush = Microsoft.UI.Xaml.Media.SolidColorBrush;
using NativeThickness = Microsoft.UI.Xaml.Thickness;
using AutomationProperties = Microsoft.UI.Xaml.Automation.AutomationProperties;
using GridLength = Microsoft.UI.Xaml.GridLength;
using GridUnitType = Microsoft.UI.Xaml.GridUnitType;
using DataTemplate = Microsoft.UI.Xaml.DataTemplate;
using ScrollBarVisibility = Microsoft.UI.Xaml.Controls.ScrollBarVisibility;
using ScrollMode = Microsoft.UI.Xaml.Controls.ScrollMode;
using Visibility = Microsoft.UI.Xaml.Visibility;

namespace SilverHamu.Owner.Controls;

internal sealed class PageJumpPopup : IDisposable
{
    private readonly NativeButton anchor;
    private readonly long count, current;
    private readonly Flyout flyout = new() { Placement = FlyoutPlacementMode.TopEdgeAlignedLeft };
    private readonly NativeTextBox input = new();
    private readonly NativeTextBlock error = new() { Foreground = new NativeBrush(Windows.UI.Color.FromArgb(255, 255, 180, 169)), TextWrapping = TextWrapping.Wrap, Visibility = Visibility.Collapsed };
    private readonly NativeButton go = new() { Content = L.Get("Go"), MinWidth = 64 };
    private readonly ItemsRepeater pages = new();
    private readonly NativeGrid panel;
    private readonly Microsoft.UI.Xaml.Controls.ScrollViewer scroll;
    private readonly PointerEventHandler wheelHandler;
    private readonly double columnStep;
    private readonly TaskCompletionSource<long?> selection = new(TaskCreationOptions.RunContinuationsAsynchronously);

    public PageJumpPopup(NativeButton anchor, long count, long current)
    {
        this.anchor = anchor; this.count = count; this.current = current;
        var root = anchor.XamlRoot ?? throw new InvalidOperationException(L.Get("CouldNotOpenPageSelection161"));
        double cellWidth = Math.Max(60, count.ToString(CultureInfo.InvariantCulture).Length * 9 + 24);
        columnStep = cellWidth + 8;
        double gridHeight = Math.Min(10, count) * 40 - 4;
        double width = Math.Min(Math.Max(340, ((count + 9) / 10) * (cellWidth + 8) - 8), Math.Max(180, root.Size.Width - 64));
        panel = new NativeGrid { Width = width, RowSpacing = 8, RequestedTheme = ElementTheme.Dark, Background = new NativeBrush(Windows.UI.Color.FromArgb(0, 0, 0, 0)) };
        AutomationProperties.SetAutomationId(panel, "PageJumpPanel");
        for (int i = 0; i < 4; i++) panel.RowDefinitions.Add(new Microsoft.UI.Xaml.Controls.RowDefinition { Height = GridLength.Auto });
        panel.Children.Add(new NativeTextBlock { Text = L.Get("GoToPageNPages", count), FontSize = 16 });
        var entry = new NativeGrid { ColumnSpacing = 8 };
        entry.ColumnDefinitions.Add(new Microsoft.UI.Xaml.Controls.ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        entry.ColumnDefinitions.Add(new Microsoft.UI.Xaml.Controls.ColumnDefinition { Width = GridLength.Auto });
        input.Text = (current + 1).ToString(CultureInfo.InvariantCulture);
        input.PlaceholderText = $"1〜{count:N0}";
        AutomationProperties.SetAutomationId(input, "PageJumpInput");
        AutomationProperties.SetName(input, L.Get("DestinationPageNumber"));
        AutomationProperties.SetAutomationId(go, "PageJumpGo");
        ButtonIcons.AddNativeIcon(go, Symbol.Forward);
        AutomationProperties.SetAutomationId(error, "PageJumpError");
        entry.Children.Add(input); entry.Children.Add(go); NativeGrid.SetColumn(go, 1);
        panel.Children.Add(entry); NativeGrid.SetRow(entry, 1);
        panel.Children.Add(error); NativeGrid.SetRow(error, 2);
        pages.ItemsSource = new PageNumbers(count);
        pages.Height = gridHeight;
        pages.ItemTemplate = (DataTemplate)XamlReader.Load("""
            <DataTemplate xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation">
                <Button Height="36" Padding="6,0" Margin="0" />
            </DataTemplate>
            """);
        pages.Layout = new UniformGridLayout
        {
            Orientation = Microsoft.UI.Xaml.Controls.Orientation.Vertical, MaximumRowsOrColumns = 10,
            MinItemWidth = cellWidth, MinItemHeight = 36, MinColumnSpacing = 8, MinRowSpacing = 4,
            ItemsStretch = UniformGridLayoutItemsStretch.None
        };
        scroll = new Microsoft.UI.Xaml.Controls.ScrollViewer
        {
            Content = pages, Width = width, Height = Math.Min(gridHeight + 20, Math.Max(80, root.Size.Height - 150)),
            HorizontalScrollMode = ScrollMode.Enabled, HorizontalScrollBarVisibility = ScrollBarVisibility.Auto,
            VerticalScrollMode = ScrollMode.Enabled, VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
            HorizontalContentAlignment = Microsoft.UI.Xaml.HorizontalAlignment.Left, ZoomMode = ZoomMode.Disabled
        };
        AutomationProperties.SetAutomationId(scroll, "PageJumpNumbers");
        panel.Children.Add(scroll); NativeGrid.SetRow(scroll, 3);
        flyout.Content = panel;
        var style = new Microsoft.UI.Xaml.Style { TargetType = typeof(FlyoutPresenter) };
        style.Setters.Add(new Microsoft.UI.Xaml.Setter(FrameworkElement.MaxWidthProperty, Math.Max(220, root.Size.Width - 16)));
        style.Setters.Add(new Microsoft.UI.Xaml.Setter(FrameworkElement.MaxHeightProperty, Math.Max(240, root.Size.Height - 16)));
        style.Setters.Add(new Microsoft.UI.Xaml.Setter(Microsoft.UI.Xaml.Controls.Control.PaddingProperty, new NativeThickness(12)));
        flyout.FlyoutPresenterStyle = style;
        pages.ElementPrepared += OnPrepared;
        go.Click += OnGo;
        input.KeyDown += OnInputKeyDown;
        flyout.Opened += OnOpened;
        flyout.Closed += OnClosed;
        wheelHandler = OnWheel;
        pages.AddHandler(UIElement.PointerWheelChangedEvent, wheelHandler, true);
        input.AddHandler(UIElement.PointerWheelChangedEvent, wheelHandler, true);
        panel.AddHandler(UIElement.PointerWheelChangedEvent, wheelHandler, true);
    }

    public Task<long?> ShowAsync() { flyout.ShowAt(anchor); return selection.Task; }

    private void OnPrepared(ItemsRepeater sender, ItemsRepeaterElementPreparedEventArgs args)
    {
        if (args.Element is not NativeButton button) return;
        long page = (long)args.Index + 1;
        if (pages.Layout is UniformGridLayout layout) button.Width = layout.MinItemWidth;
        button.Content = page.ToString(CultureInfo.InvariantCulture);
        button.Tag = page;
        button.Background = new NativeBrush(page - 1 == current ? Windows.UI.Color.FromArgb(255, 89, 217, 179) : Windows.UI.Color.FromArgb(255, 24, 35, 45));
        button.Foreground = new NativeBrush(page - 1 == current ? Windows.UI.Color.FromArgb(255, 16, 23, 30) : Windows.UI.Color.FromArgb(255, 226, 238, 244));
        AutomationProperties.SetAutomationId(button, $"PageJump{page}");
        AutomationProperties.SetName(button, L.Get("GoToPageN", page));
        button.Click -= OnQuickPage;
        button.Click += OnQuickPage;
    }

    private void OnQuickPage(object sender, RoutedEventArgs args)
    {
        if (sender is NativeButton { Tag: long page }) Choose(page - 1);
    }
    private void OnGo(object sender, RoutedEventArgs args)
    {
        try { Choose(PageNumbers.Parse(input.Text, count)); }
        catch (ArgumentException error) { this.error.Text = error.Message; this.error.Visibility = Visibility.Visible; }
    }
    private void OnInputKeyDown(object sender, KeyRoutedEventArgs args)
    { if (args.Key == VirtualKey.Enter) { OnGo(sender, args); args.Handled = true; } }
    private void Choose(long page) { selection.TrySetResult(page); flyout.Hide(); }
    private void OnOpened(object? sender, object args)
    { input.Focus(FocusState.Programmatic); input.SelectAll(); }
    private void OnClosed(object? sender, object args) => selection.TrySetResult(null);

    private void OnWheel(object sender, PointerRoutedEventArgs args)
    {
        if (args.Handled && ReferenceEquals(sender, panel)) return;
        var properties = args.GetCurrentPoint(panel).Properties;
        if (!properties.IsHorizontalMouseWheel && scroll.ScrollableHeight > 1)
            scroll.ChangeView(null, Math.Clamp(scroll.VerticalOffset - properties.MouseWheelDelta, 0, scroll.ScrollableHeight), null, true);
        else
        {
            double direction = properties.IsHorizontalMouseWheel ? 1 : -1;
            double offset = scroll.HorizontalOffset + direction * properties.MouseWheelDelta / 120d * columnStep * 3;
            scroll.ChangeView(Math.Clamp(offset, 0, scroll.ScrollableWidth), null, null, true);
        }
        args.Handled = true;
    }

    public void Dispose()
    {
        flyout.Hide();
        pages.ElementPrepared -= OnPrepared; go.Click -= OnGo; input.KeyDown -= OnInputKeyDown;
        flyout.Opened -= OnOpened; flyout.Closed -= OnClosed;
        pages.RemoveHandler(UIElement.PointerWheelChangedEvent, wheelHandler);
        input.RemoveHandler(UIElement.PointerWheelChangedEvent, wheelHandler);
        panel.RemoveHandler(UIElement.PointerWheelChangedEvent, wheelHandler);
        flyout.Content = null;
    }
}
