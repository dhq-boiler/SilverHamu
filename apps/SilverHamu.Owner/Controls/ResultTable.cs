using System.ComponentModel;
using System.Diagnostics;
using System.Text;
using Microsoft.Maui.Handlers;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Data;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Markup;
using Microsoft.UI.Xaml.Media;
using Windows.System;
using NativeGrid = Microsoft.UI.Xaml.Controls.Grid;
using NativeGridLength = Microsoft.UI.Xaml.GridLength;
using SolidColorBrush = Microsoft.UI.Xaml.Media.SolidColorBrush;
using ListView = Microsoft.UI.Xaml.Controls.ListView;
using DispatcherQueueTimer = Microsoft.UI.Dispatching.DispatcherQueueTimer;
using DataTemplate = Microsoft.UI.Xaml.DataTemplate;
using Binding = Microsoft.UI.Xaml.Data.Binding;
using ListViewSelectionMode = Microsoft.UI.Xaml.Controls.ListViewSelectionMode;
using ScrollMode = Microsoft.UI.Xaml.Controls.ScrollMode;
using ScrollBarVisibility = Microsoft.UI.Xaml.Controls.ScrollBarVisibility;

namespace SilverHamu.Owner.Controls;

public sealed class ResultTable : View
{
    internal event Action? Changed;
    internal IReadOnlyList<string> Columns { get; private set; } = [];
    internal IReadOnlyList<string[]> Rows { get; private set; } = [];
    internal double[] Widths { get; private set; } = [];
    internal string EmptyText { get; private set; } = "";
    internal IReadOnlyList<long> RowNumbers { get; private set; } = [];

    public void SetData(IReadOnlyList<string> columns, IReadOnlyList<string[]> rows, double[] widths, string emptyText, IReadOnlyList<long> rowNumbers)
    {
        Columns = columns; Rows = rows; Widths = widths; EmptyText = emptyText; RowNumbers = rowNumbers;
        Changed?.Invoke();
    }
}

public sealed class ResultTableHandler : ViewHandler<ResultTable, ResultTableHost>
{
    public ResultTableHandler() : base(ViewMapper) { }
    protected override ResultTableHost CreatePlatformView() => new();
    protected override void ConnectHandler(ResultTableHost platformView)
    {
        base.ConnectHandler(platformView);
        VirtualView.Changed += Update;
        Update();
    }
    protected override void DisconnectHandler(ResultTableHost platformView)
    {
        VirtualView.Changed -= Update;
        platformView.Dispose();
        base.DisconnectHandler(platformView);
    }
    private void Update() => PlatformView.SetData(VirtualView);
}

public sealed class ResultColumn(double width) : INotifyPropertyChanged
{
    public event PropertyChangedEventHandler? PropertyChanged;
    public NativeGridLength GridWidth => new(width);
    public void SetWidth(double value)
    {
        width = value;
        PropertyChanged?.Invoke(this, new(nameof(GridWidth)));
    }
}

public sealed class ResultLayout(double[] widths) : INotifyPropertyChanged
{
    private double rowNumberWidth = 80;
    public event PropertyChangedEventHandler? PropertyChanged;
    public double[] Widths { get; } = widths;
    public ResultColumn[] Columns { get; } = widths.Select(x => new ResultColumn(x)).ToArray();
    public NativeGridLength RowNumberGridWidth => new(rowNumberWidth);
    public double TotalWidth => rowNumberWidth + Widths.Sum();
    public void SetRowNumberWidth(int digits)
    {
        var width = Math.Max(80, digits * 8 + 24);
        if (width == rowNumberWidth) return;
        rowNumberWidth = width;
        PropertyChanged?.Invoke(this, new(nameof(RowNumberGridWidth)));
        PropertyChanged?.Invoke(this, new(nameof(TotalWidth)));
    }
    public void SetWidth(int index, double value)
    {
        Widths[index] = value;
        Columns[index].SetWidth(value);
        PropertyChanged?.Invoke(this, new(nameof(TotalWidth)));
    }
}

public sealed record ResultCell(string Text, string AutomationId);
public sealed record ResultDisplayRow(ResultLayout Layout, ResultCell[] Cells, SolidColorBrush Background, string RowNumber, string RowNumberAutomationId);

public sealed class ResultTableHost : NativeGrid, IDisposable
{
    private readonly ListView list = new() { SelectionMode = ListViewSelectionMode.None, IsItemClickEnabled = false, Padding = new(0), BorderThickness = new(0) };
    private readonly DispatcherQueueTimer resizeTimer;
    private ResultLayout? layout;
    private IReadOnlyList<string> titles = [];
    private IReadOnlyList<string[]> values = [];
    private int resizingColumn;
    private double pendingDelta;
    private Stopwatch? displayWatch;
    private static SolidColorBrush Brush(byte r, byte g, byte b) => new(Windows.UI.Color.FromArgb(255, r, g, b));

    public ResultTableHost()
    {
        ScrollViewer.SetHorizontalScrollMode(list, ScrollMode.Enabled);
        ScrollViewer.SetHorizontalScrollBarVisibility(list, ScrollBarVisibility.Auto);
        ScrollViewer.SetVerticalScrollBarVisibility(list, ScrollBarVisibility.Auto);
        list.ItemContainerStyle = (Microsoft.UI.Xaml.Style)XamlReader.Load("<Style xmlns='http://schemas.microsoft.com/winfx/2006/xaml/presentation' TargetType='ListViewItem'><Setter Property='Padding' Value='0'/><Setter Property='Margin' Value='0'/><Setter Property='MinHeight' Value='40'/><Setter Property='HorizontalContentAlignment' Value='Stretch'/></Style>");
        list.ItemsPanel = (ItemsPanelTemplate)XamlReader.Load("<ItemsPanelTemplate xmlns='http://schemas.microsoft.com/winfx/2006/xaml/presentation'><ItemsStackPanel Orientation='Vertical'/></ItemsPanelTemplate>");
        Children.Add(list);
        list.ContainerContentChanging += (_, args) =>
        {
            if (!args.InRecycleQueue)
                Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(args.ItemContainer, $"行 {((ResultDisplayRow)args.Item).RowNumber}");
        };
        list.LayoutUpdated += (_, _) =>
        {
            if (displayWatch is null || list.ItemsPanelRoot is not { } panel) return;
            Debug.WriteLine($"ResultTable layout: rows={values.Count}, realizedChildren={panel.Children.Count}, elapsed={displayWatch.Elapsed.TotalMilliseconds:F1}ms");
            displayWatch = null;
        };
        resizeTimer = DispatcherQueue.CreateTimer();
        resizeTimer.Interval = TimeSpan.FromMilliseconds(16);
        resizeTimer.IsRepeating = false;
        resizeTimer.Tick += (_, _) => FlushResize();
    }

    internal void SetData(ResultTable data)
    {
        FlushResize();
        var watch = Stopwatch.StartNew();
        displayWatch = Stopwatch.StartNew();
        values = data.Rows;
        if (layout is null || !ReferenceEquals(layout.Widths, data.Widths))
        {
            layout = new(data.Widths);
            titles = data.Columns;
            list.ItemTemplate = BuildTemplate(titles.Count);
            list.Header = titles.Count == 0 ? null : BuildHeader();
        }
        var alternate = Brush(20, 31, 40);
        var background = Brush(16, 23, 30);
        layout.SetRowNumberWidth(data.RowNumbers.Count == 0 ? 1 : data.RowNumbers.Max().ToString(System.Globalization.CultureInfo.InvariantCulture).Length);
        list.ItemsSource = values.Select((row, index) => new ResultDisplayRow(layout,
            row.Select((text, col) => new ResultCell(text, $"ResultCell{index}_{col}")).ToArray(), index % 2 == 0 ? alternate : background,
            data.RowNumbers[index].ToString(System.Globalization.CultureInfo.InvariantCulture), $"ResultRowNumber{index}")).ToArray();
        list.Footer = values.Count == 0 ? new TextBlock { Text = data.EmptyText, Foreground = Brush(148, 168, 184), Margin = new(24), FontSize = 13 } : null;
        Debug.WriteLine($"ResultTable: rows={values.Count}, columns={titles.Count}, apply={watch.Elapsed.TotalMilliseconds:F1}ms");
    }

    private static DataTemplate BuildTemplate(int columns)
    {
        var xaml = new StringBuilder("<DataTemplate xmlns='http://schemas.microsoft.com/winfx/2006/xaml/presentation'><Grid Height='40' Width='{Binding Layout.TotalWidth}' HorizontalAlignment='Left' Background='{Binding Background}'><Grid.ColumnDefinitions><ColumnDefinition Width='{Binding Layout.RowNumberGridWidth}'/>");
        for (var i = 0; i < columns; i++) xaml.Append($"<ColumnDefinition Width='{{Binding Layout.Columns[{i}].GridWidth}}'/>");
        xaml.Append("</Grid.ColumnDefinitions>");
        xaml.Append("<Border BorderBrush='#34434F' BorderThickness='0,0,1,1' Background='#18232D'><TextBlock Text='{Binding RowNumber}' AutomationProperties.AutomationId='{Binding RowNumberAutomationId}' Margin='12,0' VerticalAlignment='Center' HorizontalAlignment='Right' Foreground='#94A8B8' FontSize='12' TextWrapping='NoWrap'/></Border>");
        for (var i = 0; i < columns; i++)
            xaml.Append($"<Border Grid.Column='{i + 1}' BorderBrush='#34434F' BorderThickness='0,0,1,1'><TextBlock Text='{{Binding Cells[{i}].Text}}' AutomationProperties.AutomationId='{{Binding Cells[{i}].AutomationId}}' ToolTipService.ToolTip='{{Binding Cells[{i}].Text}}' Margin='12,0' VerticalAlignment='Center' Foreground='White' FontSize='12' TextWrapping='NoWrap' TextTrimming='CharacterEllipsis' MaxLines='1'/></Border>");
        xaml.Append("</Grid></DataTemplate>");
        return (DataTemplate)XamlReader.Load(xaml.ToString());
    }

    private NativeGrid BuildHeader()
    {
        var header = new NativeGrid { Height = 36, HorizontalAlignment = Microsoft.UI.Xaml.HorizontalAlignment.Left, Background = Brush(24, 35, 45) };
        header.SetBinding(WidthProperty, new Binding { Source = layout, Path = new PropertyPath(nameof(ResultLayout.TotalWidth)) });
        var numberColumn = new Microsoft.UI.Xaml.Controls.ColumnDefinition();
        BindingOperations.SetBinding(numberColumn, Microsoft.UI.Xaml.Controls.ColumnDefinition.WidthProperty, new Binding { Source = layout, Path = new PropertyPath(nameof(ResultLayout.RowNumberGridWidth)) });
        header.ColumnDefinitions.Add(numberColumn);
        var numberHeader = new TextBlock { Text = "行番号", Foreground = Brush(148, 168, 184), FontSize = 12, Margin = new(12, 0, 12, 0), VerticalAlignment = Microsoft.UI.Xaml.VerticalAlignment.Center, HorizontalAlignment = Microsoft.UI.Xaml.HorizontalAlignment.Right };
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetAutomationId(numberHeader, "ResultRowNumberHeader");
        header.Children.Add(numberHeader);
        for (var i = 0; i < titles.Count; i++)
        {
            var definition = new Microsoft.UI.Xaml.Controls.ColumnDefinition();
            BindingOperations.SetBinding(definition, Microsoft.UI.Xaml.Controls.ColumnDefinition.WidthProperty, new Binding { Source = layout!.Columns[i], Path = new PropertyPath(nameof(ResultColumn.GridWidth)) });
            header.ColumnDefinitions.Add(definition);
            var label = new TextBlock { Text = titles[i], Foreground = Brush(148, 168, 184), FontSize = 12, FontWeight = Microsoft.UI.Text.FontWeights.Bold, Margin = new(12, 0, 12, 0), VerticalAlignment = Microsoft.UI.Xaml.VerticalAlignment.Center, TextTrimming = Microsoft.UI.Xaml.TextTrimming.CharacterEllipsis, TextWrapping = Microsoft.UI.Xaml.TextWrapping.NoWrap };
            Microsoft.UI.Xaml.Automation.AutomationProperties.SetAutomationId(label, $"ResultHeader{i}");
            ToolTipService.SetToolTip(label, titles[i]);
            NativeGrid.SetColumn(label, i + 1); header.Children.Add(label);
            var grip = new ColumnResizeHost { Width = 10, HorizontalAlignment = Microsoft.UI.Xaml.HorizontalAlignment.Right };
            var index = i;
            Microsoft.UI.Xaml.Automation.AutomationProperties.SetAutomationId(grip.Handle, $"ColumnResize{i}Grip");
            Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(grip.Handle, $"{titles[i]} の列幅を変更");
            grip.Handle.DragDelta += (_, args) => QueueResize(index, args.HorizontalChange);
            grip.Handle.DragCompleted += (_, _) => FlushResize();
            grip.Handle.AddHandler(DoubleTappedEvent, new DoubleTappedEventHandler((_, args) => { AutoFit(index); args.Handled = true; }), true);
            grip.Handle.KeyDown += (_, args) =>
            {
                if (args.Key == VirtualKey.Enter) AutoFit(index);
                else if (args.Key is VirtualKey.Left or VirtualKey.Right) SetWidth(index, layout!.Widths[index] + (args.Key == VirtualKey.Left ? -16 : 16));
                else return;
                args.Handled = true;
            };
            ToolTipService.SetToolTip(grip, "ドラッグで列幅を変更。ダブルクリックまたはEnterで表示中の内容に合わせます。");
            NativeGrid.SetColumn(grip, i + 1); header.Children.Add(grip);
        }
        return header;
    }

    private void QueueResize(int index, double delta)
    {
        if (resizingColumn != index) FlushResize();
        resizingColumn = index; pendingDelta += delta;
        if (!resizeTimer.IsRunning) resizeTimer.Start();
    }
    private void FlushResize()
    {
        resizeTimer.Stop();
        if (layout is not null && pendingDelta != 0) SetWidth(resizingColumn, layout.Widths[resizingColumn] + pendingDelta);
        pendingDelta = 0;
    }
    private void SetWidth(int index, double width)
    {
        if (layout is null || !double.IsFinite(width)) return;
        width = Math.Clamp(width, 80, 1200);
        if (Math.Abs(width - layout.Widths[index]) >= 0.1) layout.SetWidth(index, width);
    }
    private void AutoFit(int index)
    {
        FlushResize();
        var measurement = new TextBlock { FontSize = 12, FontWeight = Microsoft.UI.Text.FontWeights.Bold, TextWrapping = Microsoft.UI.Xaml.TextWrapping.NoWrap };
        double Measure(string value)
        {
            measurement.Text = value;
            measurement.Measure(new Windows.Foundation.Size(double.PositiveInfinity, double.PositiveInfinity));
            return Math.Ceiling(measurement.DesiredSize.Width) + 26;
        }
        var width = Math.Max(80, Measure(titles[index]));
        measurement.FontWeight = Microsoft.UI.Text.FontWeights.Normal;
        foreach (var row in values)
        {
            width = Math.Max(width, Measure(row[index]));
            if (width >= 1200) break;
        }
        SetWidth(index, width);
    }
    public void Dispose() => resizeTimer.Stop();
}
