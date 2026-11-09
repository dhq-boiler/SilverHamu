using Microsoft.Extensions.Logging;

namespace Catdb.Owner;

public static class MauiProgram
{
	public static MauiApp CreateMauiApp()
	{
		var builder = MauiApp.CreateBuilder();
		builder
			.UseMauiApp<App>()
			.ConfigureMauiHandlers(handlers => handlers.AddHandler<Controls.SecretEntry, Controls.SecretEntryHandler>())
			.ConfigureMauiHandlers(handlers => handlers.AddHandler<Controls.ColumnResizeHandle, Controls.ColumnResizeHandleHandler>())
			.ConfigureMauiHandlers(handlers => handlers.AddHandler<Controls.ResultTable, Controls.ResultTableHandler>())
			.ConfigureFonts(fonts =>
			{
				fonts.AddFont("OpenSans-Regular.ttf", "OpenSansRegular");
				fonts.AddFont("OpenSans-Semibold.ttf", "OpenSansSemibold");
			});

#if DEBUG
		builder.Logging.AddDebug();
#endif

		return builder.Build();
	}
}
