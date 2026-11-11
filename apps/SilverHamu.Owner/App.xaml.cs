using Microsoft.Extensions.DependencyInjection;

namespace SilverHamu.Owner;

public partial class App : Application
{
	public App()
	{
		InitializeComponent();
		Services.L.SetLanguage(Services.L.ResolveLanguage(Preferences.Default.Get("Owner.Language", ""), System.Globalization.CultureInfo.CurrentUICulture.Name));
	}

	protected override Window CreateWindow(IActivationState? activationState)
	{
		UserAppTheme = AppTheme.Dark;
		return new Window(new MainPage()) { Title = "SilverHamu Owner", Width = 1440, Height = 920, MinimumWidth = 1000, MinimumHeight = 700 };
	}
}
