using System.Diagnostics;
using System.Runtime.InteropServices.WindowsRuntime;
using Microsoft.UI.Xaml.Media.Imaging;
using Windows.Graphics.Imaging;
using NativeImage = Microsoft.UI.Xaml.Controls.Image;

namespace SilverHamu.Owner.Controls;

internal sealed class LoadingAnimation : Image
{
    private sealed record Frame(WriteableBitmap Image, TimeSpan Duration);
    private Task<Frame[]>? loading;
    private Frame[] frames = [];
    private IDispatcherTimer? timer;
    private NativeImage? native;
    private bool playing, loaded;
    private int position;

    public LoadingAnimation()
    {
        // WinUIのGIFはIsPlayingがtrueのまま描画が止まるため、自動再生に任せない。
        HandlerChanged += async (_, _) => await AttachAsync();
        Loaded += (_, _) => { loaded = true; UpdatePlayback(); };
        Unloaded += (_, _) => { loaded = false; timer?.Stop(); };
        HandlerChanging += (_, _) => { timer?.Stop(); native = null; };
    }

    public void SetPlaying(bool value)
    {
        if (playing == value) return;
        playing = value;
        position = 0;
        UpdatePlayback();
    }

    private async Task AttachAsync()
    {
        var target = Handler?.PlatformView as NativeImage;
        if (target is null) return;
        try
        {
            frames = await (loading ??= LoadFramesAsync());
            if (!ReferenceEquals(Handler?.PlatformView, target)) return;
            native = target;
            if (timer is null)
            {
                timer = Dispatcher.CreateTimer();
                timer.Tick += (_, _) =>
                {
                    position = (position + 1) % frames.Length;
                    ShowFrame();
                };
            }
            UpdatePlayback();
        }
        catch (Exception error) { Debug.WriteLine($"Loading animation: {error}"); }
    }

    private void UpdatePlayback()
    {
        timer?.Stop();
        if (native is null || frames.Length == 0) return;
        ShowFrame();
        if (playing && loaded) timer?.Start();
    }

    private void ShowFrame()
    {
        if (native is null || timer is null) return;
        native.Source = frames[position].Image;
        timer.Interval = frames[position].Duration;
    }

    private static async Task<Frame[]> LoadFramesAsync()
    {
        using var file = await FileSystem.OpenAppPackageFileAsync("silverhamu_loading.gif");
        using var stream = file.AsRandomAccessStream();
        var decoder = await BitmapDecoder.CreateAsync(BitmapDecoder.GifDecoderId, stream);
        var result = new Frame[decoder.FrameCount];
        for (uint i = 0; i < decoder.FrameCount; i++)
        {
            var frame = await decoder.GetFrameAsync(i);
            var pixels = await frame.GetPixelDataAsync(BitmapPixelFormat.Bgra8, BitmapAlphaMode.Premultiplied,
                new BitmapTransform(), ExifOrientationMode.IgnoreExifOrientation, ColorManagementMode.DoNotColorManage);
            var image = new WriteableBitmap((int)frame.PixelWidth, (int)frame.PixelHeight);
            using (var buffer = image.PixelBuffer.AsStream()) await buffer.WriteAsync(pixels.DetachPixelData());
            var metadata = await frame.BitmapProperties.GetPropertiesAsync(["/grctlext/Delay"]);
            double milliseconds = metadata.TryGetValue("/grctlext/Delay", out var delay) ? Convert.ToDouble(delay.Value) * 10 : 100;
            result[i] = new Frame(image, TimeSpan.FromMilliseconds(Math.Max(20, milliseconds)));
        }
        if (result.Length == 0) throw new InvalidDataException(Services.L.Get("LoadingFramesMissing"));
        return result;
    }
}
