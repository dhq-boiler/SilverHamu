using Microsoft.Maui.Handlers;
using Microsoft.UI.Input;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Input;
using Windows.System;

namespace Catdb.Owner.Controls;

public sealed class ColumnResizeHandle : View
{
    public string ColumnTitle { get; init; } = "";
    public event Action<double>? Resize;
    public event Action? AutoFit;
    internal void ChangeWidth(double delta) => Resize?.Invoke(delta);
    internal void FitWidth() => AutoFit?.Invoke();
}

public sealed class ColumnResizeHandleHandler : ViewHandler<ColumnResizeHandle, ColumnResizeHost>
{
    public ColumnResizeHandleHandler() : base(ViewMapper) { }
    protected override ColumnResizeHost CreatePlatformView() => new();
    protected override void ConnectHandler(ColumnResizeHost platformView)
    {
        base.ConnectHandler(platformView);
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(platformView.Handle, $"{VirtualView.ColumnTitle} の列幅を変更");
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetAutomationId(platformView.Handle, VirtualView.AutomationId + "Grip");
        platformView.Handle.DragDelta += OnDragDelta;
        platformView.Handle.KeyDown += OnKeyDown;
        platformView.Handle.AddHandler(Microsoft.UI.Xaml.UIElement.DoubleTappedEvent, new DoubleTappedEventHandler(OnDoubleTapped), true);
    }
    protected override void DisconnectHandler(ColumnResizeHost platformView)
    {
        platformView.Handle.DragDelta -= OnDragDelta;
        platformView.Handle.KeyDown -= OnKeyDown;
        platformView.Handle.RemoveHandler(Microsoft.UI.Xaml.UIElement.DoubleTappedEvent, new DoubleTappedEventHandler(OnDoubleTapped));
        base.DisconnectHandler(platformView);
    }
    private void OnDragDelta(object sender, DragDeltaEventArgs args) => VirtualView.ChangeWidth(args.HorizontalChange);
    private void OnDoubleTapped(object sender, DoubleTappedRoutedEventArgs args)
    {
        VirtualView.FitWidth();
        args.Handled = true;
    }
    private void OnKeyDown(object sender, KeyRoutedEventArgs args)
    {
        if (args.Key == VirtualKey.Enter)
        {
            VirtualView.FitWidth();
            args.Handled = true;
            return;
        }
        if (args.Key is not (VirtualKey.Left or VirtualKey.Right)) return;
        VirtualView.ChangeWidth(args.Key == VirtualKey.Left ? -16 : 16);
        args.Handled = true;
    }
}

public sealed class ColumnResizeHost : Microsoft.UI.Xaml.Controls.Grid
{
    public Thumb Handle { get; }
    public ColumnResizeHost()
    {
        ProtectedCursor = InputSystemCursor.Create(InputSystemCursorShape.SizeWestEast);
        Handle = new Thumb { IsTabStop = true, Template = (Microsoft.UI.Xaml.Controls.ControlTemplate)Microsoft.UI.Xaml.Markup.XamlReader.Load("<ControlTemplate xmlns='http://schemas.microsoft.com/winfx/2006/xaml/presentation' TargetType='Thumb'><Grid Background='Transparent'><Border Width='2' Background='#597487' HorizontalAlignment='Center'/></Grid></ControlTemplate>") };
        Children.Add(Handle);
    }
}
