using System.Globalization;
using SilverHamu.Owner.Services;

namespace SilverHamu.Owner;

internal static class LocalizationBindings
{
    public static T Localized<T>(this T target, BindableProperty property, string key) where T : BindableObject
    {
        Bind(target, property, key);
        return target;
    }

    public static void Bind(BindableObject target, BindableProperty property, string key, params object?[] arguments) =>
        target.SetBinding(property, new Binding(nameof(L.Language), source: L.Instance, converter: new Translation(key, arguments)));

    private sealed class Translation(string key, object?[] arguments) : IValueConverter
    {
        public object Convert(object? value, Type targetType, object? parameter, CultureInfo culture) => L.Get(key, arguments);
        public object ConvertBack(object? value, Type targetType, object? parameter, CultureInfo culture) => throw new NotSupportedException();
    }
}
