using Microsoft.Maui.Handlers;
using Microsoft.UI.Xaml.Controls;

namespace Catdb.Owner.Controls;

public sealed class SecretEntry : View
{
    public static readonly BindableProperty TextProperty = BindableProperty.Create(nameof(Text), typeof(string), typeof(SecretEntry), "", BindingMode.TwoWay);
    public string Text { get => (string)GetValue(TextProperty); set => SetValue(TextProperty, value); }
}

public sealed class SecretEntryHandler : ViewHandler<SecretEntry, PasswordBox>
{
    public static readonly IPropertyMapper<SecretEntry, SecretEntryHandler> SecretMapper = new PropertyMapper<SecretEntry, SecretEntryHandler>(ViewMapper)
    {
        [nameof(SecretEntry.Text)] = (handler, view) => handler.PlatformView.Password = view.Text
    };

    public SecretEntryHandler() : base(SecretMapper) { }
    protected override PasswordBox CreatePlatformView() => new() { PlaceholderText = "認証トークン（保存されません）", PasswordRevealMode = PasswordRevealMode.Hidden };
    protected override void ConnectHandler(PasswordBox platformView)
    {
        base.ConnectHandler(platformView);
        platformView.PasswordChanged += OnPasswordChanged;
    }
    protected override void DisconnectHandler(PasswordBox platformView)
    {
        platformView.PasswordChanged -= OnPasswordChanged;
        base.DisconnectHandler(platformView);
    }
    private void OnPasswordChanged(object sender, Microsoft.UI.Xaml.RoutedEventArgs args) => VirtualView.Text = PlatformView.Password;
}
