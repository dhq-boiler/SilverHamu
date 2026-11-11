using System.Collections;
using System.Globalization;

namespace SilverHamu.Owner.Services;

internal sealed class PageNumbers : IReadOnlyList<int>, IList
{
    public PageNumbers(long count)
    {
        if (count < 1 || count > int.MaxValue) throw new ArgumentOutOfRangeException(nameof(count));
        Count = (int)count;
    }

    public int Count { get; }
    public int this[int index] => index >= 0 && index < Count ? index + 1 : throw new ArgumentOutOfRangeException(nameof(index));
    public bool IsReadOnly => true;
    public bool IsFixedSize => true;
    public bool IsSynchronized => false;
    public object SyncRoot => this;
    object? IList.this[int index] { get => this[index]; set => throw new NotSupportedException(); }
    public bool Contains(object? value) => value is int page && page >= 1 && page <= Count;
    public int IndexOf(object? value) => Contains(value) ? (int)value! - 1 : -1;
    public void CopyTo(Array array, int index)
    { for (int i = 0; i < Count; i++) array.SetValue(this[i], index + i); }
    public IEnumerator<int> GetEnumerator()
    { for (int i = 0; i < Count; i++) yield return this[i]; }
    IEnumerator IEnumerable.GetEnumerator() => GetEnumerator();
    public int Add(object? value) => throw new NotSupportedException();
    public void Clear() => throw new NotSupportedException();
    public void Insert(int index, object? value) => throw new NotSupportedException();
    public void Remove(object? value) => throw new NotSupportedException();
    public void RemoveAt(int index) => throw new NotSupportedException();

    public static long Parse(string? text, long count)
    {
        if (!long.TryParse(text?.Trim(), NumberStyles.None, CultureInfo.InvariantCulture, out long page) || page < 1 || page > count)
            throw new ArgumentException(L.Get("EnterAPageNumberFromToN", count));
        return page - 1;
    }
}
