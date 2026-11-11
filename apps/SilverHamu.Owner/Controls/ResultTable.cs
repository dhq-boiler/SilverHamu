using System.ComponentModel;
using SilverHamu.Owner.Services;
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

public sealed record ResultEditValue(string Text, bool IsNull, bool CanBeNull);
public enum ResultRowState { Original, Added, Deleted }

public sealed class ResultEditRequest(int row, int column, string text, bool isNull) : EventArgs
{
    public int Row { get; } = row;
    public int Column { get; } = column;
    public string Text { get; } = text;
    public bool IsNull { get; } = isNull;
    public bool Accepted { get; set; }
    public string? Error { get; set; }
}

public sealed class ResultTable : View
{
    internal event Action? Changed;
    internal event Action? EditingChanged;
    public event EventHandler<ResultEditRequest>? EditRequested;
    public event EventHandler? EditCompleted;
    public event EventHandler? PendingEditChanged;
    public Action<bool>? ChangeHistory { get; set; }
    public Action? AddRowRequested { get; set; }
    public Action<int[]>? DeleteRowsRequested { get; set; }
    public Action<int[]>? DuplicateRowsRequested { get; set; }
    internal Action<int, int>? BeginEditHandler { get; set; }
    public void BeginCellEdit(int row, int column) => BeginEditHandler?.Invoke(row, column);
    public Func<int, int, ResultEditValue>? ReadEditValue { get; set; }
    internal Func<bool>? CommitEditHandler { get; set; }
    public bool TryCommitPendingEdit() => CommitEditHandler?.Invoke() ?? true;
    public bool HasPendingEdit { get; private set; }
    internal void SetPendingEdit(bool value) { HasPendingEdit = value; PendingEditChanged?.Invoke(this, EventArgs.Empty); }
    private bool isEditingEnabled;
    public bool IsEditingEnabled
    {
        get => isEditingEnabled;
        set { if (isEditingEnabled == value) return; isEditingEnabled = value; EditingChanged?.Invoke(); }
    }
    internal ResultEditRequest CommitEdit(int row, int column, string text, bool isNull)
    {
        var request = new ResultEditRequest(row, column, text, isNull);
        EditRequested?.Invoke(this, request);
        return request;
    }
    internal void NotifyEditCompleted() => EditCompleted?.Invoke(this, EventArgs.Empty);
    internal IReadOnlyList<string> Columns { get; private set; } = [];
    internal IReadOnlyList<string[]> Rows { get; private set; } = [];
    internal double[] Widths { get; private set; } = [];
    internal string EmptyText { get; private set; } = "";
    internal IReadOnlyList<long> RowNumbers { get; private set; } = [];
    internal IReadOnlySet<ResultAddress> ModifiedCells { get; private set; } = new HashSet<ResultAddress>();
    internal IReadOnlyList<ResultRowState> RowStates { get; private set; } = [];
    internal IReadOnlyList<string> RowLabels { get; private set; } = [];

    internal void SetData(IReadOnlyList<string> columns, IReadOnlyList<string[]> rows, double[] widths, string emptyText, IReadOnlyList<long> rowNumbers, IReadOnlySet<ResultAddress>? modifiedCells = null, IReadOnlyList<ResultRowState>? rowStates = null, IReadOnlyList<string>? rowLabels = null)
    {
        Columns = columns; Rows = rows; Widths = widths; EmptyText = emptyText; RowNumbers = rowNumbers;
        ModifiedCells = modifiedCells ?? new HashSet<ResultAddress>();
        RowStates = rowStates ?? Enumerable.Repeat(ResultRowState.Original, rows.Count).ToArray();
        RowLabels = rowLabels ?? rowNumbers.Select(n => n.ToString(System.Globalization.CultureInfo.InvariantCulture)).ToArray();
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
        VirtualView.EditingChanged += UpdateEditing;
        VirtualView.CommitEditHandler = PlatformView.CommitInlineEdit;
        VirtualView.BeginEditHandler = PlatformView.BeginInlineEdit;
        Update();
    }
    protected override void DisconnectHandler(ResultTableHost platformView)
    {
        VirtualView.Changed -= Update;
        VirtualView.EditingChanged -= UpdateEditing;
        VirtualView.CommitEditHandler = null;
        VirtualView.BeginEditHandler = null;
        platformView.Dispose();
        base.DisconnectHandler(platformView);
    }
    private void Update() => PlatformView.SetData(VirtualView);
    private void UpdateEditing() => PlatformView.RefreshEditing();
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
    public double HorizontalOffset { get; private set; }
    public void SetHorizontalOffset(double offset)
    {
        if (Math.Abs(HorizontalOffset - offset) < 0.1) return;
        HorizontalOffset = offset;
        PropertyChanged?.Invoke(this, new(nameof(HorizontalOffset)));
    }
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

public sealed class ResultCell : INotifyPropertyChanged
{
    private readonly ResultSelection selection;
    private static readonly SolidColorBrush SelectedBrush = new(Windows.UI.Color.FromArgb(255, 36, 94, 104));
    private static readonly SolidColorBrush RowBrush = new(Windows.UI.Color.FromArgb(255, 24, 35, 45));
    private static readonly SolidColorBrush ClearBrush = new(Windows.UI.Color.FromArgb(0, 0, 0, 0));
    private static readonly SolidColorBrush FocusBrush = new(Windows.UI.Color.FromArgb(255, 125, 228, 209));
    private static readonly SolidColorBrush GridBrush = new(Windows.UI.Color.FromArgb(255, 52, 67, 79));
    private static readonly SolidColorBrush PendingBrush = new(Windows.UI.Color.FromArgb(255, 87, 62, 23));
    private static readonly SolidColorBrush PendingSelectedBrush = new(Windows.UI.Color.FromArgb(255, 111, 86, 40));
    private static readonly SolidColorBrush PendingOutline = new(Windows.UI.Color.FromArgb(255, 224, 170, 73));
    private static readonly SolidColorBrush AddedBrush = new(Windows.UI.Color.FromArgb(255, 26, 67, 47));
    private static readonly SolidColorBrush AddedSelectedBrush = new(Windows.UI.Color.FromArgb(255, 38, 93, 64));
    private static readonly SolidColorBrush DeletedBrush = new(Windows.UI.Color.FromArgb(255, 85, 38, 42));
    private static readonly SolidColorBrush DeletedSelectedBrush = new(Windows.UI.Color.FromArgb(255, 112, 51, 57));
    internal ResultCell(string text, string id, int row, int column, ResultSelection selection, bool modified = false, ResultRowState state = ResultRowState.Original)
    { Text = text; AutomationId = id; Row = row; Column = column; this.selection = selection; IsModified = modified; State = state; }
    public string Text { get; }
    public string AutomationId { get; }
    public int Row { get; }
    public int Column { get; }
    public bool IsModified { get; }
    public ResultRowState State { get; }
    public Windows.UI.Text.TextDecorations Decorations => State == ResultRowState.Deleted ? Windows.UI.Text.TextDecorations.Strikethrough : Windows.UI.Text.TextDecorations.None;
    public bool IsSelected => Column < 0 ? selection.ContainsRow(Row) : selection.Contains(Row, Column);
    public bool IsTabStop => selection.Focus is { } focus ? focus == new ResultAddress(Row, Column) : Row == 0 && Column == 0;
    public SolidColorBrush Background => State == ResultRowState.Deleted ? IsSelected ? DeletedSelectedBrush : DeletedBrush
        : State == ResultRowState.Added ? IsSelected ? AddedSelectedBrush : AddedBrush
        : IsModified ? IsSelected ? PendingSelectedBrush : PendingBrush : IsSelected ? SelectedBrush : Column < 0 ? RowBrush : ClearBrush;
    public SolidColorBrush Outline => selection.Focus == new ResultAddress(Row, Column) ? FocusBrush : IsModified ? PendingOutline : GridBrush;
    private string PendingDescription => State == ResultRowState.Added ? L.Get("EditRowAdded") : State == ResultRowState.Deleted ? L.Get("EditRowDeleted") : IsModified ? L.Get("EditCellPending") : "";
    public string SelectionDescription => L.Get(IsSelected ? "ResultCellSelected" : "ResultCellNotSelected") + (PendingDescription.Length > 0 ? " / " + PendingDescription : "");
    public string Description => PendingDescription.Length > 0 ? Text + " — " + PendingDescription : Text;
    public event PropertyChangedEventHandler? PropertyChanged;
    internal void RefreshSelection() => PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(null));
}

public sealed record ResultDisplayRow(ResultLayout Layout, ResultCell[] Cells, SolidColorBrush Background, string RowNumber, ResultCell RowHeader);

public sealed partial class ResultTableHost : NativeGrid, IDisposable
{
    private readonly ListView list = new() { SelectionMode = ListViewSelectionMode.None, IsItemClickEnabled = false, Padding = new(0), BorderThickness = new(0) };
    private readonly Microsoft.UI.Xaml.Controls.Canvas stickyHeader = new() { Height = 36, Visibility = Microsoft.UI.Xaml.Visibility.Collapsed, Background = Brush(24, 35, 45) };
    private Microsoft.UI.Xaml.Controls.ScrollViewer? resultScroller;
    private readonly DispatcherQueueTimer resizeTimer;
    private ResultLayout? layout;
    private IReadOnlyList<string> titles = [];
    private IReadOnlyList<string[]> values = [];
    private string headerLanguage = "";
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
        InitializeSelection();
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetAutomationId(stickyHeader, "ResultStickyHeader");
        stickyHeader.SizeChanged += (_, _) => stickyHeader.Clip = new RectangleGeometry { Rect = new(0, 0, stickyHeader.ActualWidth, stickyHeader.ActualHeight) };
        list.Loaded += AttachResultScroller;
        stickyHeader.PointerWheelChanged += (_, args) =>
        {
            if (resultScroller is null) return;
            var properties = args.GetCurrentPoint(stickyHeader).Properties;
            if (properties.IsHorizontalMouseWheel)
                resultScroller.ChangeView(Math.Clamp(resultScroller.HorizontalOffset + properties.MouseWheelDelta, 0, resultScroller.ScrollableWidth), null, null, true);
            else
                resultScroller.ChangeView(null, Math.Clamp(resultScroller.VerticalOffset - properties.MouseWheelDelta, 0, resultScroller.ScrollableHeight), null, true);
            args.Handled = true;
        };
        list.ContainerContentChanging += ConfigureRow;
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
        EndInlineEdit();
        editSource = data;
        FlushResize();
        var watch = Stopwatch.StartNew();
        displayWatch = Stopwatch.StartNew();
        values = data.Rows;
        pointerSelecting = suppressPointerClick = false;
        dragLast = null;
        selection.Reset(values.Count, data.Columns.Count);
        if (layout is null || !ReferenceEquals(layout.Widths, data.Widths) || headerLanguage != L.Instance.Language)
        {
            headerLanguage = L.Instance.Language;
            layout = new(data.Widths);
            titles = data.Columns;
            list.ItemTemplate = BuildTemplate(titles.Count);
            // ListView.Headerは本文と一緒に縦スクロールするため、固定行へ分離する。
            stickyHeader.Children.Clear();
            stickyHeader.Visibility = titles.Count == 0 ? Microsoft.UI.Xaml.Visibility.Collapsed : Microsoft.UI.Xaml.Visibility.Visible;
            if (titles.Count > 0) stickyHeader.Children.Add(BuildHeader());
            SyncHeaderPosition();
        }
        var alternate = Brush(20, 31, 40);
        var background = Brush(16, 23, 30);
        layout.SetRowNumberWidth(data.RowNumbers.Count == 0 ? 1 : data.RowNumbers.Max().ToString(System.Globalization.CultureInfo.InvariantCulture).Length);
        list.ItemsSource = values.Select((row, index) => new ResultDisplayRow(layout,
            row.Select((text, col) => new ResultCell(text, $"ResultCell{index}_{col}", index, col, selection, data.ModifiedCells.Contains(new(index, col)), data.RowStates[index])).ToArray(), index % 2 == 0 ? alternate : background,
            data.RowLabels[index],
            new ResultCell(data.RowLabels[index], $"ResultRowNumber{index}", index, -1, selection, state: data.RowStates[index]))).ToArray();
        list.Footer = values.Count == 0 ? new TextBlock { Text = data.EmptyText, Foreground = Brush(148, 168, 184), Margin = new(24), FontSize = 13 } : null;
        UpdateSelectionCommands();
        Debug.WriteLine($"ResultTable: rows={values.Count}, columns={titles.Count}, apply={watch.Elapsed.TotalMilliseconds:F1}ms");
    }

    private static DataTemplate BuildTemplate(int columns)
    {
        var xaml = new StringBuilder("<DataTemplate xmlns='http://schemas.microsoft.com/winfx/2006/xaml/presentation'><Grid Height='40' Width='{Binding Layout.TotalWidth}' HorizontalAlignment='Left' Background='{Binding Background}'><Grid.ColumnDefinitions><ColumnDefinition Width='{Binding Layout.RowNumberGridWidth}'/>");
        for (var i = 0; i < columns; i++) xaml.Append($"<ColumnDefinition Width='{{Binding Layout.Columns[{i}].GridWidth}}'/>");
        xaml.Append("</Grid.ColumnDefinitions>");
        xaml.Append("<Button Canvas.ZIndex='1' Tag='{Binding RowHeader}' AutomationProperties.AutomationId='{Binding RowHeader.AutomationId}' AutomationProperties.ItemStatus='{Binding RowHeader.SelectionDescription}' Background='{Binding RowHeader.Background}' BorderBrush='{Binding RowHeader.Outline}' BorderThickness='1' CornerRadius='0' Padding='0' HorizontalAlignment='Stretch' VerticalAlignment='Stretch' MinWidth='0' MinHeight='0' IsTabStop='{Binding RowHeader.IsTabStop}' HorizontalContentAlignment='Stretch' VerticalContentAlignment='Stretch'><Button.RenderTransform><TranslateTransform X='{Binding Layout.HorizontalOffset}'/></Button.RenderTransform><TextBlock Text='{Binding RowNumber}' Margin='12,0' VerticalAlignment='Center' HorizontalAlignment='Right' Foreground='#94A8B8' FontSize='12' TextWrapping='NoWrap'/></Button>");
        for (var i = 0; i < columns; i++)
            xaml.Append($"<Button Grid.Column='{i + 1}' Tag='{{Binding Cells[{i}]}}' AutomationProperties.AutomationId='{{Binding Cells[{i}].AutomationId}}' AutomationProperties.ItemStatus='{{Binding Cells[{i}].SelectionDescription}}' Background='{{Binding Cells[{i}].Background}}' BorderBrush='{{Binding Cells[{i}].Outline}}' BorderThickness='1' CornerRadius='0' Padding='0' HorizontalAlignment='Stretch' VerticalAlignment='Stretch' MinWidth='0' MinHeight='0' IsTabStop='{{Binding Cells[{i}].IsTabStop}}' HorizontalContentAlignment='Stretch' VerticalContentAlignment='Stretch' ToolTipService.ToolTip='{{Binding Cells[{i}].Description}}' AutomationProperties.HelpText='{{Binding Cells[{i}].Description}}'><TextBlock Text='{{Binding Cells[{i}].Text}}' TextDecorations='{{Binding Cells[{i}].Decorations}}' Margin='12,0' VerticalAlignment='Center' Foreground='White' FontSize='12' TextWrapping='NoWrap' TextTrimming='CharacterEllipsis' MaxLines='1'/></Button>");
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
        var numberHeader = new TextBlock { Text = L.Get("RowNumber"), Foreground = Brush(148, 168, 184), FontSize = 12, Margin = new(12, 0, 12, 0), VerticalAlignment = Microsoft.UI.Xaml.VerticalAlignment.Center, HorizontalAlignment = Microsoft.UI.Xaml.HorizontalAlignment.Right };
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetAutomationId(numberHeader, "ResultRowNumberHeader");
        var corner = new Microsoft.UI.Xaml.Controls.Border { Child = numberHeader, Background = Brush(24, 35, 45), BorderBrush = Brush(52, 67, 79), BorderThickness = new(0, 0, 1, 1) };
        var frozenOffset = new TranslateTransform();
        BindingOperations.SetBinding(frozenOffset, TranslateTransform.XProperty, new Binding { Source = layout, Path = new PropertyPath(nameof(ResultLayout.HorizontalOffset)) });
        corner.RenderTransform = frozenOffset;
        Microsoft.UI.Xaml.Controls.Canvas.SetZIndex(corner, 1);
        header.Children.Add(corner);
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
            Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(grip.Handle, L.Get("ResizeColumn", titles[i]));
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
            ToolTipService.SetToolTip(grip, L.Get("ResizeColumnHelp"));
            NativeGrid.SetColumn(grip, i + 1); header.Children.Add(grip);
        }
        return header;
    }

    private void AttachResultScroller(object sender, RoutedEventArgs args)
    {
        list.ApplyTemplate();
        if (resultScroller is not null) resultScroller.ViewChanged -= OnResultViewChanged;
        resultScroller = Descendants(list).OfType<Microsoft.UI.Xaml.Controls.ScrollViewer>().FirstOrDefault();
        if (resultScroller is not null) resultScroller.ViewChanged += OnResultViewChanged;
        SyncHeaderPosition();
    }

    private void OnResultViewChanged(object? sender, ScrollViewerViewChangedEventArgs args) => SyncHeaderPosition();

    private void SyncHeaderPosition()
    {
        var offset = resultScroller?.HorizontalOffset ?? 0;
        layout?.SetHorizontalOffset(offset);
        if (stickyHeader.Children.Count > 0)
            Microsoft.UI.Xaml.Controls.Canvas.SetLeft(stickyHeader.Children[0], -offset);
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
    public void Dispose()
    {
        resizeTimer.Stop();
        list.Loaded -= AttachResultScroller;
        if (resultScroller is not null) resultScroller.ViewChanged -= OnResultViewChanged;
    }
}
