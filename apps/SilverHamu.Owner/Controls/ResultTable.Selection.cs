using Microsoft.UI.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using SilverHamu.Owner.Services;
using Windows.ApplicationModel.DataTransfer;
using Windows.System;
using Windows.UI.Core;
using NativeButton = Microsoft.UI.Xaml.Controls.Button;
using NativeTextBlock = Microsoft.UI.Xaml.Controls.TextBlock;
using NativeStackPanel = Microsoft.UI.Xaml.Controls.StackPanel;
using AutomationProperties = Microsoft.UI.Xaml.Automation.AutomationProperties;
using RowDefinition = Microsoft.UI.Xaml.Controls.RowDefinition;
using MenuFlyout = Microsoft.UI.Xaml.Controls.MenuFlyout;
using MenuFlyoutItem = Microsoft.UI.Xaml.Controls.MenuFlyoutItem;
using MenuFlyoutSeparator = Microsoft.UI.Xaml.Controls.MenuFlyoutSeparator;
using ScrollMode = Microsoft.UI.Xaml.Controls.ScrollMode;
using ScrollBarVisibility = Microsoft.UI.Xaml.Controls.ScrollBarVisibility;
using DataPackage = Windows.ApplicationModel.DataTransfer.DataPackage;
using DataPackageOperation = Windows.ApplicationModel.DataTransfer.DataPackageOperation;

namespace SilverHamu.Owner.Controls;

public sealed partial class ResultTableHost
{
    private readonly ResultSelection selection = new();
    private readonly NativeButton copySelection = new(), copyHeaders = new(), selectAll = new();
    private readonly NativeButton editCell = new(), addRow = new(), duplicateRows = new(), deleteRows = new();
    private readonly MenuFlyoutItem editMenu = new(), addRowMenu = new(), duplicateRowsMenu = new(), deleteRowsMenu = new();
    private ResultTable? editSource;
    private readonly NativeTextBlock selectionStatus = new() { FontSize = 12, VerticalAlignment = Microsoft.UI.Xaml.VerticalAlignment.Center };
    private readonly MenuFlyoutItem copyMenu = new(), headersMenu = new(), allMenu = new();
    private bool pointerSelecting, suppressPointerClick, draggingRows;
    private ResultAddress? dragLast;
    private ResultAddress? lastCellPress;
    private long lastCellPressTime;

    [System.Runtime.InteropServices.DllImport("user32.dll")]
    private static extern uint GetDoubleClickTime();

    private void InitializeSelection()
    {
        RowDefinitions.Add(new RowDefinition { Height = Microsoft.UI.Xaml.GridLength.Auto });
        RowDefinitions.Add(new RowDefinition { Height = Microsoft.UI.Xaml.GridLength.Auto });
        RowDefinitions.Add(new RowDefinition { Height = new Microsoft.UI.Xaml.GridLength(1, Microsoft.UI.Xaml.GridUnitType.Star) });
        SetRow(stickyHeader, 1);
        Children.Add(stickyHeader);
        SetRow(list, 2);
        var commands = new NativeStackPanel { Orientation = Microsoft.UI.Xaml.Controls.Orientation.Horizontal, Spacing = 8, Margin = new(8, 4, 8, 4) };
        foreach (var button in new[] { addRow, duplicateRows, deleteRows, editCell, copySelection, copyHeaders, selectAll })
        {
            button.FontSize = 12;
            button.Padding = new(8, 4, 8, 4);
            button.Background = Brush(24, 35, 45);
            button.Foreground = Brush(226, 238, 244);
            commands.Children.Add(button);
        }
        commands.Children.Add(selectionStatus);
        Children.Add(new Microsoft.UI.Xaml.Controls.ScrollViewer
        {
            Content = commands, HorizontalScrollMode = ScrollMode.Enabled, HorizontalScrollBarVisibility = ScrollBarVisibility.Auto,
            VerticalScrollMode = ScrollMode.Disabled, VerticalScrollBarVisibility = ScrollBarVisibility.Disabled
        });
        AutomationProperties.SetAutomationId(list, "ResultSelectionGrid");
        AutomationProperties.SetAutomationId(editCell, "EditResultCell");
        AutomationProperties.SetAutomationId(editMenu, "ResultEditMenu");
        editCell.Click += (_, _) => RequestCellEdit();
        editMenu.Click += (_, _) => RequestCellEdit();
        editMenu.Icon = new SymbolIcon(Symbol.Edit);
        AutomationProperties.SetAutomationId(addRow, "AddResultRow");
        AutomationProperties.SetAutomationId(duplicateRows, "DuplicateResultRows");
        AutomationProperties.SetAutomationId(duplicateRowsMenu, "DuplicateResultRowsMenu");
        AutomationProperties.SetAutomationId(deleteRows, "DeleteResultRows");
        AutomationProperties.SetAutomationId(addRowMenu, "AddResultRowMenu");
        AutomationProperties.SetAutomationId(deleteRowsMenu, "DeleteResultRowsMenu");
        duplicateRowsMenu.Icon = new SymbolIcon(Symbol.Copy);
        duplicateRows.Click += (_, _) => RequestRowDuplicate();
        duplicateRowsMenu.Click += (_, _) => RequestRowDuplicate();
        addRowMenu.Icon = new SymbolIcon(Symbol.Add); deleteRowsMenu.Icon = new SymbolIcon(Symbol.Delete);
        addRow.Click += (_, _) => editSource?.AddRowRequested?.Invoke();
        addRowMenu.Click += (_, _) => editSource?.AddRowRequested?.Invoke();
        deleteRows.Click += (_, _) => RequestRowDelete();
        deleteRowsMenu.Click += (_, _) => RequestRowDelete();
        AutomationProperties.SetAutomationId(copySelection, "CopyResultSelection");
        AutomationProperties.SetAutomationId(copyHeaders, "CopyResultWithHeaders");
        AutomationProperties.SetAutomationId(selectAll, "SelectAllResultCells");
        AutomationProperties.SetAutomationId(selectionStatus, "ResultSelectionStatus");
        AutomationProperties.SetLiveSetting(selectionStatus, Microsoft.UI.Xaml.Automation.Peers.AutomationLiveSetting.Polite);
        copySelection.Click += (_, _) => CopySelection(false);
        copyHeaders.Click += (_, _) => CopySelection(true);
        selectAll.Click += (_, _) => { selection.SelectAll(); RefreshSelection(); };
        copyMenu.Icon = new SymbolIcon(Symbol.Copy);
        headersMenu.Icon = new SymbolIcon(Symbol.Copy);
        allMenu.Icon = new SymbolIcon(Symbol.SelectAll);
        AutomationProperties.SetAutomationId(copyMenu, "ResultCopyMenu");
        AutomationProperties.SetAutomationId(headersMenu, "ResultCopyHeadersMenu");
        AutomationProperties.SetAutomationId(allMenu, "ResultSelectAllMenu");
        copyMenu.Click += (_, _) => CopySelection(false);
        headersMenu.Click += (_, _) => CopySelection(true);
        allMenu.Click += (_, _) => { selection.SelectAll(); RefreshSelection(); };
        var menu = new MenuFlyout();
        menu.Items.Add(addRowMenu); menu.Items.Add(duplicateRowsMenu); menu.Items.Add(deleteRowsMenu); menu.Items.Add(editMenu);
        menu.Items.Add(copyMenu); menu.Items.Add(headersMenu); menu.Items.Add(new MenuFlyoutSeparator()); menu.Items.Add(allMenu);
        list.ContextFlyout = menu;
        list.PreviewKeyDown += OnResultKeyDown;
        list.AddHandler(PointerPressedEvent, new PointerEventHandler(OnSelectionPressed), true);
        list.AddHandler(PointerMovedEvent, new PointerEventHandler(OnSelectionMoved), true);
        list.AddHandler(PointerReleasedEvent, new PointerEventHandler(OnSelectionReleased), true);
        list.AddHandler(PointerCanceledEvent, new PointerEventHandler(OnSelectionReleased), true);
        list.AddHandler(RightTappedEvent, new RightTappedEventHandler(OnSelectionRightTapped), true);
    }

    private void ConfigureRow(ListViewBase sender, ContainerContentChangingEventArgs args)
    {
        if (args.InRecycleQueue || args.Item is not ResultDisplayRow row) return;
        if (args.Phase == 0)
        {
            AutomationProperties.SetName(args.ItemContainer, L.Get("ResultRow", row.RowNumber));
            args.RegisterUpdateCallback(1, ConfigureRow);
            return;
        }
        RefreshRow(row);
        if (args.ItemContainer.ContentTemplateRoot is not { } root) return;
        foreach (var button in Descendants(root).OfType<NativeButton>())
        {
            if (button.Tag is not ResultCell) continue;
            button.Click -= OnCellClick;
            button.Click += OnCellClick;
            button.PreviewKeyDown -= OnResultKeyDown;
            button.PreviewKeyDown += OnResultKeyDown;
            button.IsDoubleTapEnabled = true;
            button.DoubleTapped -= OnCellDoubleTapped;
            button.DoubleTapped += OnCellDoubleTapped;
        }
    }

    private static IEnumerable<DependencyObject> Descendants(DependencyObject root)
    {
        yield return root;
        for (int i = 0; i < VisualTreeHelper.GetChildrenCount(root); i++)
            foreach (var child in Descendants(VisualTreeHelper.GetChild(root, i))) yield return child;
    }

    private static NativeButton? CellButtonAt(DependencyObject? source)
    {
        for (var current = source; current is not null; current = VisualTreeHelper.GetParent(current))
            if (current is NativeButton { Tag: ResultCell } button) return button;
        return null;
    }

    private static ResultCell? CellAt(DependencyObject? source) => CellButtonAt(source)?.Tag as ResultCell;

    private static bool IsKeyPressed(VirtualKey key) => (InputKeyboardSource.GetKeyStateForCurrentThread(key) & CoreVirtualKeyStates.Down) != 0;

    private void Choose(ResultCell cell, bool extend, bool toggle)
    {
        if (cell.Column < 0) selection.SelectRow(cell.Row, extend, toggle);
        else selection.SelectCell(cell.Row, cell.Column, extend, toggle);
        RefreshSelection();
    }

    private void OnCellClick(object sender, RoutedEventArgs args)
    {
        if (ReferenceEquals(sender, inlineButton) || IsInsideInline(args.OriginalSource as DependencyObject)) return;
        if (!CommitInlineEdit()) return;
        if (suppressPointerClick) { suppressPointerClick = false; return; }
        if (sender is NativeButton { Tag: ResultCell cell } button)
        {
            Choose(cell, IsKeyPressed(VirtualKey.Shift), IsKeyPressed(VirtualKey.Control));
            button.Focus(FocusState.Programmatic);
        }
    }

    private void OnSelectionPressed(object sender, PointerRoutedEventArgs args)
    {
        if (IsInsideInline(args.OriginalSource as DependencyObject) || !CommitInlineEdit()) return;
        if (!args.GetCurrentPoint(list).Properties.IsLeftButtonPressed || CellButtonAt(args.OriginalSource as DependencyObject) is not { Tag: ResultCell cell } button) return;
        var address = new ResultAddress(cell.Row, cell.Column);
        long now = Environment.TickCount64;
        bool doubleClick = args.Pointer.PointerDeviceType == PointerDeviceType.Mouse && lastCellPress == address && now - lastCellPressTime <= GetDoubleClickTime();
        lastCellPress = address; lastCellPressTime = now;
        Choose(cell, IsKeyPressed(VirtualKey.Shift), IsKeyPressed(VirtualKey.Control));
        if (doubleClick && cell.Column >= 0 && editSource?.IsEditingEnabled == true && !IsKeyPressed(VirtualKey.Shift) && !IsKeyPressed(VirtualKey.Control))
        {
            lastCellPress = null;
            BeginInlineEdit(cell.Row, cell.Column);
            args.Handled = true;
            return;
        }
        button.Focus(FocusState.Programmatic);
        list.CapturePointer(args.Pointer);
        pointerSelecting = suppressPointerClick = true;
        draggingRows = cell.Column < 0;
        dragLast = new(cell.Row, cell.Column);
        args.Handled = true;
    }

    private void OnSelectionMoved(object sender, PointerRoutedEventArgs args)
    {
        if (!pointerSelecting) return;
        var cell = HitCell(args.GetCurrentPoint(list).Position);
        if (cell is null || dragLast == new ResultAddress(cell.Row, cell.Column)) return;
        dragLast = new(cell.Row, cell.Column);
        if (draggingRows) selection.SelectRow(cell.Row, extend: true);
        else selection.SelectCell(cell.Row, Math.Max(0, cell.Column), extend: true);
        RefreshSelection();
        args.Handled = true;
    }

    private ResultCell? HitCell(Windows.Foundation.Point point)
    {
        if (list.ItemsPanelRoot is not { } panel) return null;
        foreach (var container in panel.Children.OfType<ListViewItem>())
        {
            var position = container.TransformToVisual(list).TransformPoint(new(0, 0));
            if (point.Y < position.Y || point.Y >= position.Y + container.ActualHeight || container.ContentTemplateRoot is not { } root) continue;
            foreach (var button in Descendants(root).OfType<NativeButton>())
            {
                var origin = button.TransformToVisual(list).TransformPoint(new(0, 0));
                if (point.X >= origin.X && point.X < origin.X + button.ActualWidth && point.Y >= origin.Y && point.Y < origin.Y + button.ActualHeight)
                    return button.Tag as ResultCell;
            }
        }
        return null;
    }

    private void OnSelectionReleased(object sender, PointerRoutedEventArgs args)
    {
        if (pointerSelecting && HitCell(args.GetCurrentPoint(list).Position) is { } cell && dragLast != new ResultAddress(cell.Row, cell.Column))
        {
            if (draggingRows) selection.SelectRow(cell.Row, extend: true);
            else selection.SelectCell(cell.Row, Math.Max(0, cell.Column), extend: true);
            RefreshSelection();
        }
        pointerSelecting = false;
        list.ReleasePointerCapture(args.Pointer);
        DispatcherQueue.TryEnqueue(Microsoft.UI.Dispatching.DispatcherQueuePriority.Low, () => suppressPointerClick = false);
    }

    private void OnSelectionRightTapped(object sender, RightTappedRoutedEventArgs args)
    {
        if (IsInsideInline(args.OriginalSource as DependencyObject) || !CommitInlineEdit()) return;
        if (CellAt(args.OriginalSource as DependencyObject) is not { } cell) return;
        bool selected = cell.Column < 0 ? selection.ContainsRow(cell.Row) : selection.Contains(cell.Row, cell.Column);
        if (!selected) Choose(cell, false, false);
    }

    private static void RefreshRow(ResultDisplayRow row)
    {
        row.RowHeader.RefreshSelection();
        foreach (var cell in row.Cells) cell.RefreshSelection();
    }

    private void RefreshSelection()
    {
        if (list.ItemsPanelRoot is { } panel)
            foreach (var container in panel.Children.OfType<ListViewItem>())
                if (container.Content is ResultDisplayRow row) RefreshRow(row);
        UpdateSelectionCommands();
    }

    private void UpdateSelectionCommands()
    {
        addRow.Content = addRowMenu.Text = L.Get("EditAddRow");
        duplicateRows.Content = duplicateRowsMenu.Text = L.Get("EditDuplicateRows");
        ButtonIcons.AddNativeIcon(duplicateRows, Symbol.Copy);
        ToolTipService.SetToolTip(duplicateRows, L.Get("EditDuplicateRowsHelp"));
        ToolTipService.SetToolTip(duplicateRowsMenu, L.Get("EditDuplicateRowsHelp"));
        AutomationProperties.SetHelpText(duplicateRows, L.Get("EditDuplicateRowsHelp"));
        deleteRows.Content = deleteRowsMenu.Text = L.Get("EditDeleteRows");
        ButtonIcons.AddNativeIcon(addRow, Symbol.Add); ButtonIcons.AddNativeIcon(deleteRows, Symbol.Delete);
        addRow.Visibility = duplicateRows.Visibility = deleteRows.Visibility = addRowMenu.Visibility = duplicateRowsMenu.Visibility = deleteRowsMenu.Visibility = editSource?.IsEditingEnabled == true ? Microsoft.UI.Xaml.Visibility.Visible : Microsoft.UI.Xaml.Visibility.Collapsed;
        addRow.IsEnabled = addRowMenu.IsEnabled = editSource?.IsEditingEnabled == true;
        duplicateRows.IsEnabled = duplicateRowsMenu.IsEnabled =
        deleteRows.IsEnabled = deleteRowsMenu.IsEnabled = editSource?.IsEditingEnabled == true && SelectedRows().Length > 0;
        editCell.Content = editMenu.Text = L.Get("EditResultCell");
        ButtonIcons.AddNativeIcon(editCell, Symbol.Edit);
        editCell.Visibility = editMenu.Visibility = editSource?.IsEditingEnabled == true ? Microsoft.UI.Xaml.Visibility.Visible : Microsoft.UI.Xaml.Visibility.Collapsed;
        editCell.IsEnabled = editMenu.IsEnabled = editSource?.IsEditingEnabled == true && selection.Focus is { Column: >= 0 } focus && editSource.RowStates[focus.Row] != ResultRowState.Deleted;
        copySelection.Content = copyMenu.Text = L.Get("CopyResultSelection");
        copyHeaders.Content = headersMenu.Text = L.Get("CopyResultWithHeaders");
        selectAll.Content = allMenu.Text = L.Get("SelectAllResultCells");
        ButtonIcons.AddNativeIcon(copySelection, Symbol.Copy);
        ButtonIcons.AddNativeIcon(copyHeaders, Symbol.Copy);
        ButtonIcons.AddNativeIcon(selectAll, Symbol.SelectAll);
        copySelection.IsEnabled = copyHeaders.IsEnabled = copyMenu.IsEnabled = headersMenu.IsEnabled = selection.Count > 0;
        selectAll.IsEnabled = allMenu.IsEnabled = values.Count > 0 && titles.Count > 0;
        var help = L.Get(editSource?.IsEditingEnabled == true ? "EditResultHelp" : "ResultSelectionHelp");
        selectionStatus.Text = selection.Count == 0 ? help : L.Get("ResultSelectionCount", selection.Count);
        ToolTipService.SetToolTip(list, help);
        AutomationProperties.SetHelpText(list, help);
    }

    internal void RefreshEditing() => UpdateSelectionCommands();

    private int[] SelectedRows() => selection.SelectedRows().Where(row => editSource?.RowStates[row] != ResultRowState.Deleted).ToArray();

    private void RequestRowDuplicate()
    {
        if (editSource?.IsEditingEnabled == true) editSource.DuplicateRowsRequested?.Invoke(SelectedRows());
    }

    private void RequestRowDelete()
    {
        if (editSource?.IsEditingEnabled != true) return;
        var rows = SelectedRows();
        if (inlineGrid is not null && rows.Contains(inlineRow)) EndInlineEdit();
        else if (!CommitInlineEdit()) return;
        editSource.DeleteRowsRequested?.Invoke(rows);
    }

    private void RequestCellEdit()
    {
        if (editSource?.IsEditingEnabled == true && selection.Focus is { Column: >= 0 } focus)
            BeginInlineEdit(focus.Row, focus.Column);
    }

    private void OnCellDoubleTapped(object sender, DoubleTappedRoutedEventArgs args)
    {
        if (IsInsideInline(args.OriginalSource as DependencyObject)) return;
        if (editSource?.IsEditingEnabled != true || sender is not NativeButton { Tag: ResultCell { Column: >= 0 } cell }) return;
        Choose(cell, false, false);
        RequestCellEdit();
        args.Handled = true;
    }

    private void CopySelection(bool includeHeaders)
    {
        if (!CommitInlineEdit() || selection.Count == 0) return;
        try
        {
            var data = new DataPackage { RequestedOperation = DataPackageOperation.Copy };
            data.SetText(selection.CopyText(values, titles, includeHeaders));
            Windows.ApplicationModel.DataTransfer.Clipboard.SetContent(data);
            Windows.ApplicationModel.DataTransfer.Clipboard.Flush();
            selectionStatus.Text = L.Get("ResultSelectionCopied", selection.Count);
        }
        catch (Exception error) { selectionStatus.Text = L.Get("ResultSelectionCopyFailed", error.Message); }
    }

    private void OnResultKeyDown(object sender, KeyRoutedEventArgs args)
    {
        if (args.Handled || inlineGrid is not null) return;
        if (args.Key == VirtualKey.Delete && editSource?.IsEditingEnabled == true) { RequestRowDelete(); args.Handled = true; return; }
        if (args.Key == VirtualKey.F2 && editSource?.IsEditingEnabled == true) { RequestCellEdit(); args.Handled = true; return; }
        bool control = IsKeyPressed(VirtualKey.Control), shift = IsKeyPressed(VirtualKey.Shift);
        if (control && (args.Key == VirtualKey.Z || args.Key == VirtualKey.Y) && editSource?.ChangeHistory is { } changeHistory)
        {
            var address = selection.Focus;
            changeHistory(args.Key == VirtualKey.Y || shift);
            DispatcherQueue.TryEnqueue(Microsoft.UI.Dispatching.DispatcherQueuePriority.Low, () =>
            {
                if (address is not { } previous || selection.RowCount == 0 || selection.ColumnCount == 0) return;
                int row = Math.Min(previous.Row, selection.RowCount - 1), column = Math.Min(previous.Column, selection.ColumnCount - 1);
                if (column < 0) selection.SelectRow(row); else selection.SelectCell(row, column);
                RefreshSelection(); list.UpdateLayout();
                Descendants(list).OfType<NativeButton>().FirstOrDefault(b => b.Tag is ResultCell c && c.Row == row && c.Column == column)?.Focus(FocusState.Programmatic);
            });
            args.Handled = true; return;
        }
        if (control && args.Key == VirtualKey.C) { CopySelection(shift); args.Handled = true; return; }
        if (control && args.Key == VirtualKey.A) { selection.SelectAll(); RefreshSelection(); args.Handled = true; return; }
        if (args.Key == VirtualKey.Escape) { selection.Clear(); RefreshSelection(); args.Handled = true; return; }
        if (selection.RowCount == 0 || selection.ColumnCount == 0) return;
        if (args.Key is not (VirtualKey.Left or VirtualKey.Right or VirtualKey.Up or VirtualKey.Down or VirtualKey.Home or VirtualKey.End)) return;
        var focus = selection.Focus ?? new ResultAddress(0, 0);
        int row = focus.Row, column = focus.Column;
        switch (args.Key)
        {
            case VirtualKey.Left: column = Math.Max(0, column - 1); break;
            case VirtualKey.Right: column = Math.Min(selection.ColumnCount - 1, column + 1); break;
            case VirtualKey.Up: row = Math.Max(0, row - 1); break;
            case VirtualKey.Down: row = Math.Min(selection.RowCount - 1, row + 1); break;
            case VirtualKey.Home: column = 0; if (control) row = 0; break;
            case VirtualKey.End: column = selection.ColumnCount - 1; if (control) row = selection.RowCount - 1; break;
        }
        if (column < 0) selection.SelectRow(row, shift);
        else selection.SelectCell(row, column, shift);
        RefreshSelection();
        list.ScrollIntoView(list.Items[row]);
        DispatcherQueue.TryEnqueue(() =>
        {
            if (list.ContainerFromIndex(row) is ListViewItem { ContentTemplateRoot: { } root })
                Descendants(root).OfType<NativeButton>().FirstOrDefault(button => button.Tag is ResultCell cell && cell.Column == column)?.Focus(FocusState.Programmatic);
        });
        args.Handled = true;
    }
}
