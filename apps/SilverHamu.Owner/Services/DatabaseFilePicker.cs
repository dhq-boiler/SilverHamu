using System.ComponentModel;
using System.Runtime.InteropServices;

namespace SilverHamu.Owner.Services;

internal static class DatabaseFilePicker
{
    public static async Task<string?> PickAsync(Microsoft.UI.Xaml.Window window, string currentPath, string title)
    {
        await Task.Yield();
        var directory = Path.IsPathFullyQualified(currentPath) ? Path.GetDirectoryName(currentPath) : null;
        var selection = new OpenFileName
        {
            Owner = WinRT.Interop.WindowNative.GetWindowHandle(window),
            Filter = L.Get("DatabaseFileFilters").Replace('|', '\0'),
            FilterIndex = Path.GetExtension(currentPath).ToLowerInvariant() switch { ".ndb" => 1, ".backup" => 2, _ => 3 },
            File = Marshal.AllocHGlobal(32768 * sizeof(char)),
            MaxFile = 32768,
            InitialDirectory = Directory.Exists(directory) ? directory : null,
            Title = title,
            Flags = 0x00080000 | 0x00001000 | 0x00000800 | 0x00000008
        };
        try
        {
            Marshal.WriteInt16(selection.File, 0);
            selection.StructSize = Marshal.SizeOf<OpenFileName>();
            if (GetOpenFileName(selection)) return Marshal.PtrToStringUni(selection.File);
            var error = CommDlgExtendedError();
            if (error != 0) throw new Win32Exception((int)error, L.Get("CouldNotOpenTheFilePickerXX", error));
            return null;
        }
        finally { Marshal.FreeHGlobal(selection.File); }
    }

    [DllImport("comdlg32.dll", EntryPoint = "GetOpenFileNameW", CharSet = CharSet.Unicode)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetOpenFileName([In, Out] OpenFileName selection);

    [DllImport("comdlg32.dll")]
    private static extern uint CommDlgExtendedError();

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private sealed class OpenFileName
    {
        public int StructSize;
        public nint Owner;
        public nint Instance;
        public string? Filter;
        public nint CustomFilter;
        public int MaxCustomFilter;
        public int FilterIndex = 1;
        public nint File;
        public int MaxFile;
        public nint FileTitle;
        public int MaxFileTitle;
        public string? InitialDirectory;
        public string? Title;
        public int Flags;
        public ushort FileOffset;
        public ushort FileExtension;
        public string? DefaultExtension;
        public nint CustomData;
        public nint Hook;
        public nint TemplateName;
        public nint Reserved;
        public int ReservedSize;
        public int ExtendedFlags;
    }
}
