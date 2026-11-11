using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using SilverHamu.Owner.Services;
using Windows.System;
using NativeButton = Microsoft.UI.Xaml.Controls.Button;
using NativeGrid = Microsoft.UI.Xaml.Controls.Grid;
using NativeTextBox = Microsoft.UI.Xaml.Controls.TextBox;
using ToggleButton = Microsoft.UI.Xaml.Controls.Primitives.ToggleButton;
using AutomationProperties = Microsoft.UI.Xaml.Automation.AutomationProperties;
using ToolTipService = Microsoft.UI.Xaml.Controls.ToolTipService;

namespace SilverHamu.Owner.Controls;

public sealed partial class ResultTableHost
{
    private NativeButton? inlineButton;
    private NativeGrid? inlineGrid;
    private NativeTextBox? inlineText;
    private ToggleButton? inlineNull;
    private object? inlineContent;
    private int inlineRow, inlineColumn;
    private bool committingInline;

    private bool IsInsideInline(DependencyObject? source)
    {
        for (var current = source; current is not null; current = VisualTreeHelper.GetParent(current))
            if (ReferenceEquals(current, inlineGrid)) return true;
        return false;
    }

    internal void BeginInlineEdit(int row, int column)
    {
        if (editSource is null || row < 0 || row >= editSource.Rows.Count || editSource.RowStates[row] == ResultRowState.Deleted) return;
        if (inlineGrid is not null && inlineRow == row && inlineColumn == column) return;
        if (!CommitInlineEdit() || editSource?.IsEditingEnabled != true || editSource.ReadEditValue is null) return;
        selection.SelectCell(row, column); RefreshSelection();
        list.ScrollIntoView(list.Items[row]);
        list.UpdateLayout();
        var button = Descendants(list).OfType<NativeButton>().FirstOrDefault(b => b.Tag is ResultCell cell && cell.Row == row && cell.Column == column);
        if (button is null) return;
        var value = editSource.ReadEditValue(row, column);
        var input = new NativeTextBox
        {
            Text = value.Text, AcceptsReturn = true, TextWrapping = Microsoft.UI.Xaml.TextWrapping.NoWrap,
            MinWidth = 0, MinHeight = 0, FontSize = 12, Padding = new(5, 2, 5, 2),
            IsEnabled = !value.IsNull, VerticalContentAlignment = Microsoft.UI.Xaml.VerticalAlignment.Center
        };
        var nullToggle = new ToggleButton
        {
            Content = "NULL", IsChecked = value.IsNull, IsEnabled = value.CanBeNull,
            MinWidth = 44, MinHeight = 32, FontSize = 11, Padding = new(4, 0, 4, 0),
            VerticalAlignment = Microsoft.UI.Xaml.VerticalAlignment.Stretch
        };
        AutomationProperties.SetAutomationId(input, "InlineResultEditor");
        AutomationProperties.SetName(input, L.Get("EditCellValue"));
        AutomationProperties.SetAutomationId(nullToggle, "InlineResultNull");
        AutomationProperties.SetName(nullToggle, L.Get("EditUseNull"));
        ToolTipService.SetToolTip(input, L.Get("EditInlineHelp"));
        ToolTipService.SetToolTip(nullToggle, L.Get(value.CanBeNull ? "EditUseNull" : "EditNullNotAllowed"));
        var panel = new NativeGrid();
        panel.ColumnDefinitions.Add(new() { Width = new(1, Microsoft.UI.Xaml.GridUnitType.Star) });
        panel.ColumnDefinitions.Add(new() { Width = Microsoft.UI.Xaml.GridLength.Auto });
        NativeGrid.SetColumn(nullToggle, 1);
        panel.Children.Add(input); panel.Children.Add(nullToggle);
        inlineButton = button; inlineContent = button.Content; inlineGrid = panel;
        inlineText = input; inlineNull = nullToggle; inlineRow = row; inlineColumn = column;
        button.Content = panel;
        nullToggle.Checked += (_, _) => input.IsEnabled = false;
        nullToggle.Unchecked += (_, _) => { input.IsEnabled = true; input.Focus(FocusState.Programmatic); };
        panel.PreviewKeyDown += OnInlineKeyDown;
        panel.LostFocus += (_, _) => DispatcherQueue.TryEnqueue(() =>
        {
            if (ReferenceEquals(inlineGrid, panel) && !IsInsideInline(Microsoft.UI.Xaml.Input.FocusManager.GetFocusedElement(XamlRoot) as DependencyObject))
                CommitInlineEdit();
        });
        editSource.SetPendingEdit(true);
        DispatcherQueue.TryEnqueue(() =>
        {
            if (!ReferenceEquals(inlineGrid, panel)) return;
            if (value.IsNull) nullToggle.Focus(FocusState.Programmatic);
            else { input.Focus(FocusState.Programmatic); input.SelectAll(); }
        });
    }

    private void OnInlineKeyDown(object sender, KeyRoutedEventArgs args)
    {
        if (args.Key == VirtualKey.Escape)
        {
            var button = inlineButton;
            EndInlineEdit(); button?.Focus(FocusState.Programmatic);
            args.Handled = true;
        }
        else if (args.Key == VirtualKey.Enter && !IsKeyPressed(VirtualKey.Shift))
        {
            CommitInlineEdit(restoreFocus: true); args.Handled = true;
        }
    }

    internal bool CommitInlineEdit() => CommitInlineEdit(restoreFocus: false);

    private bool CommitInlineEdit(bool restoreFocus)
    {
        if (inlineGrid is null || committingInline) return true;
        committingInline = true;
        try
        {
            var source = editSource;
            var request = source?.CommitEdit(inlineRow, inlineColumn, inlineText!.Text, inlineNull!.IsChecked == true);
            if (request?.Accepted != true)
            {
                selectionStatus.Text = request?.Error ?? L.Get("EditRefreshRequired");
                AutomationProperties.SetHelpText(inlineText!, selectionStatus.Text);
                ToolTipService.SetToolTip(inlineText!, selectionStatus.Text);
                inlineText!.BorderBrush = Brush(240, 110, 110);
                (inlineText.IsEnabled ? (Microsoft.UI.Xaml.Controls.Control)inlineText : inlineNull!).Focus(FocusState.Programmatic);
                return false;
            }
            int row = inlineRow, column = inlineColumn;
            EndInlineEdit();
            source!.NotifyEditCompleted();
            if (row < selection.RowCount && column < selection.ColumnCount)
            {
                selection.SelectCell(row, column); RefreshSelection();
                if (restoreFocus)
                    DispatcherQueue.TryEnqueue(Microsoft.UI.Dispatching.DispatcherQueuePriority.Low, () =>
                    {
                        list.UpdateLayout();
                        Descendants(list).OfType<NativeButton>().FirstOrDefault(b => b.Tag is ResultCell c && c.Row == row && c.Column == column)?.Focus(FocusState.Programmatic);
                    });
            }
            return true;
        }
        finally { committingInline = false; }
    }

    private void EndInlineEdit()
    {
        var button = inlineButton;
        var content = inlineContent;
        inlineButton = null; inlineContent = null; inlineGrid = null; inlineText = null; inlineNull = null;
        if (button is not null) button.Content = content;
        editSource?.SetPendingEdit(false);
    }
}
