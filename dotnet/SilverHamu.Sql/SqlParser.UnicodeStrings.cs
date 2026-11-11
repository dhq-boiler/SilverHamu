using System.Globalization;
using System.Text;

namespace SilverHamu.Sql;

public sealed partial class SqlParser
{
    private static string DecodeUnicodeString(string value, int position)
    {
        var decoded = new StringBuilder();
        for (int i = 0; i < value.Length; i++)
        {
            if (value[i] != '\\') { decoded.Append(value[i]); continue; }
            if (++i >= value.Length) throw new SqlException($"SQL position {position + 1}: incomplete Unicode escape.");
            if (value[i] == '\\') { decoded.Append('\\'); continue; }
            bool extended = value[i] == '+';
            if (extended) i++;
            int digits = extended ? 6 : 4;
            if (i + digits > value.Length || !int.TryParse(value.AsSpan(i, digits), NumberStyles.AllowHexSpecifier, CultureInfo.InvariantCulture, out int code))
                throw new SqlException($"SQL position {position + 1}: invalid Unicode escape.");
            if (extended)
            {
                if (!Rune.IsValid(code)) throw new SqlException($"SQL position {position + 1}: invalid Unicode scalar.");
                decoded.Append(char.ConvertFromUtf32(code));
            }
            else decoded.Append((char)code);
            i += digits - 1;
        }
        return decoded.ToString();
    }
}
