using Microsoft.UI.Input;
using Microsoft.UI.Xaml.Input;
using SilverHamu.Owner.Services;
using Windows.System;
using Windows.UI.Core;

namespace SilverHamu.Owner;

public sealed partial class MainPage
{
    private readonly IDispatcherTimer editScriptTimer;
    private readonly Label editPreviewStatus = Text("", 11, color: Muted);
    private Microsoft.UI.Xaml.Controls.TextBox? historyEditor;
    private bool changingEditText, editScriptPending;

    private void SetEditText(string text)
    {
        changingEditText = true;
        try
        {
            editor.Text = documents[activeDocument].Text = text;
            if (editor.Handler?.PlatformView is Microsoft.UI.Xaml.Controls.TextBox native)
            {
                native.AcceptsReturn = true;
                native.Text = text.ReplaceLineEndings("\r\n");
            }
        }
        finally { changingEditText = false; }
    }

    private void OnEditScriptChanged(object? sender, TextChangedEventArgs args)
    {
        if (changingEditText || documents[activeDocument].Edit is not { IsCurrent: true } session) { UpdateMode(); return; }
        editScriptTimer.Stop();
        editScriptPending = (editor.Text ?? "").ReplaceLineEndings("\n") != session.Script;
        if (editScriptPending) editScriptTimer.Start();
        UpdateMode();
    }

    private void SynchronizeEditScript(bool render = true)
    {
        editScriptTimer.Stop(); editScriptPending = false;
        if (documents[activeDocument].Edit is not { IsCurrent: true } session) return;
        string sql = session.Synchronize(editor.Text ?? "");
        if (sql != (editor.Text ?? "").ReplaceLineEndings("\n")) SetEditText(sql);
        else documents[activeDocument].Text = sql;
        result = session.Result;
        if (render) RenderRows();
        UpdateMode();
    }

    private Task ChangeEditHistoryAsync(bool redo)
    {
        var session = documents[activeDocument].Edit;
        if (session is not { IsCurrent: true }) return Task.CompletedTask;
        SetEditText(redo ? session.Redo() : session.Undo());
        result = session.Result;
        RenderRows(); UpdateMode();
        return Task.CompletedTask;
    }

    private void OnEditHistoryKeyDown(object sender, KeyRoutedEventArgs args)
    {
        if (documents[activeDocument].Edit is not { IsCurrent: true } || args.Handled) return;
        bool control = (InputKeyboardSource.GetKeyStateForCurrentThread(VirtualKey.Control) & CoreVirtualKeyStates.Down) != 0;
        bool shift = (InputKeyboardSource.GetKeyStateForCurrentThread(VirtualKey.Shift) & CoreVirtualKeyStates.Down) != 0;
        if (!control || args.Key is not (VirtualKey.Z or VirtualKey.Y)) return;
        args.Handled = true;
        completionController?.Hide();
        _ = RunActionAsync(() => ChangeEditHistoryAsync(args.Key == VirtualKey.Y || shift));
    }

    private void UpdateEditHistoryControls()
    {
        var session = documents[activeDocument].Edit;
        foreach (var button in guarded.Where(b => b.AutomationId is "UndoEdit" or "RedoEdit"))
        {
            button.IsVisible = session is not null;
            button.IsEnabled = !busy && session is { IsCurrent: true } && (button.AutomationId == "UndoEdit"
                ? session.CanUndo || editScriptPending || resultTable.HasPendingEdit : session.CanRedo);
        }
        editPreviewStatus.IsVisible = session is not null;
        if (session is null) return;
        editPreviewStatus.Text = editScriptPending ? L.Get("EditPreviewUpdating") : session.PreviewError is { } error
            ? L.Get("EditPreviewInvalid", error) : session.IncompleteRowCount > 0 ? L.Get("EditIncompleteRows", session.IncompleteRowCount)
            : L.Get("EditPendingCount", session.ModifiedCellCount, session.AddedRowCount, session.DeletedRowCount, session.PendingCount);
        editPreviewStatus.TextColor = session.PreviewError is null ? Color.FromArgb("#E0AA49") : Color.FromArgb("#FFB4A9");
        ToolTipProperties.SetText(editPreviewStatus, editPreviewStatus.Text);
    }
}
