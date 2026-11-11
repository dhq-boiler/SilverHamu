from pathlib import Path
from PIL import Image, ImageDraw, ImageFont
import json

source = Path(__file__).resolve().parent / 'original.gif'
work = Path(__file__).resolve().parents[4] / 'artifacts/owner-animation-review'
work.mkdir(parents=True, exist_ok=True)
order = [6, 7, 0, 1, 2, 3, 4, 5]
original = Image.open(source)
frames, encoded, durations = [], [], []
for index in order:
    original.seek(index)
    frame = original.convert('RGBA')
    frames.append(frame)
    durations.append(original.info['duration'])
    pixels = list(frame.getdata())
    colors = sorted({p[:3] for p in pixels if p[3]})
    assert len(colors) <= 255
    indices = {rgb: i + 1 for i, rgb in enumerate(colors)}
    image = Image.frombytes('P', frame.size, bytes(indices[p[:3]] if p[3] else 0 for p in pixels))
    palette = [0, 0, 0] + [channel for rgb in colors for channel in rgb]
    image.putpalette(palette + [0] * (768 - len(palette)))
    image.info['transparency'] = 0
    encoded.append(image)

gif = work / 'original-restored-loop.gif'
encoded[0].save(gif, save_all=True, append_images=encoded[1:], duration=durations,
                loop=0, disposal=2, transparency=0, optimize=False)
with Image.open(gif) as check:
    assert check.n_frames == 8 and check.info['loop'] == 0
    for i, expected in enumerate(frames):
        check.seek(i)
        assert check.info['duration'] == durations[i] == 110
        for actual, prior in zip(check.convert('RGBA').getdata(), expected.getdata()):
            assert actual[3] == prior[3]
            if prior[3]:
                assert actual[:3] == prior[:3]

fontpath = next(p for p in [Path('C:/Windows/Fonts/YuGothM.ttc'), Path('C:/Windows/Fonts/meiryo.ttc')] if p.exists())
font = ImageFont.truetype(str(fontpath), 16)
small = ImageFont.truetype(str(fontpath), 13)
preview = Image.new('RGB', (960, 580), '#10171e')
draw = ImageDraw.Draw(preview)
draw.text((16, 12), '元の絵を保持 · 4段階 × 各2コマ · 1周0.88秒', font=font, fill='#e2eef4')
draw.text((16, 39), '再生順：各列を上から下へ → 次の列。番号は再生順、括弧内は元GIFのコマ。', font=small, fill='#94a8b8')
names = ['脚をたたむ', '蹴り出す', '脚を伸ばす', '着地へ戻る']
for column, name in enumerate(names):
    draw.text((column * 240 + 12, 70), name, font=font, fill='#59d9b3')
    for row in range(2):
        i = column * 2 + row
        x, y = column * 240 + 8, 98 + row * 238
        draw.rounded_rectangle((x, y, x + 224, y + 228), radius=7, fill='#18232d')
        frame = frames[i].copy()
        frame.thumbnail((208, 208), Image.Resampling.LANCZOS)
        preview.paste(frame, (x + 8, y), frame)
        draw.text((x + 10, y + 207), f'{i+1:02d}（元コマ{order[i]+1}）· 110ms', font=small, fill='#e2eef4')
preview.save(work / 'original-restored-preview.png')
manifest = {'source': 'original.gif', 'source_frame_count': 12, 'selected_frames': [i+1 for i in order],
            'frame_count': 8, 'duration_ms': 880, 'original_pixels_preserved': True,
            'interpolated_frames': 0, 'frames_per_phase': 2}
(work / 'original-restored-manifest.json').write_text(json.dumps(manifest, indent=2), encoding='utf-8')
print(json.dumps(manifest))
