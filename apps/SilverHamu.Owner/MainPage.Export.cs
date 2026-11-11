using System.Globalization;
using SilverHamu.Owner.Services;
using SilverHamu.Owner.Controls;
using SilverHamu.Sql;

namespace SilverHamu.Owner;

public sealed partial class MainPage
{
    private readonly Grid csvOverlay = new() { IsVisible = false, BackgroundColor = Color.FromArgb("#E010171E"), AutomationId = "CsvExportDialog" };
    private readonly RadioButton csvAll = new() { GroupName = "CsvExportScope", AutomationId = "CsvAll" };
    private readonly RadioButton csvPage = new() { GroupName = "CsvExportScope", AutomationId = "CsvPage" };
    private readonly RadioButton csvRange = new RadioButton { GroupName = "CsvExportScope", AutomationId = "CsvRange" }.Localized(RadioButton.ContentProperty, "SpecifyARowRange");
    private readonly Entry csvFirst = Input("CsvFirstRow", "1");
    private readonly Entry csvLast = Input("CsvLastRow", "1");
    private readonly Label csvSummary = Text("", 13);
    private readonly Label csvError = Text("", 12, color: Color.FromArgb("#FFB4A9"));
    private TaskCompletionSource<CsvExportRange?>? csvSelection;
    private long csvTotal, csvCurrentPage;
    private int csvPageSize;

    private void BuildCsvDialog()
    {
        var panel = new VerticalStackLayout { WidthRequest = 580, Padding = 28, Spacing = 12, BackgroundColor = Surface };
        panel.Add(LocalizedText("CSVExportRange", 22, true));
        panel.Add(csvSummary);
        foreach (var radio in new[] { csvAll, csvPage, csvRange })
        {
            radio.TextColor = Colors.White;
            panel.Add(radio);
            radio.CheckedChanged += (_, _) => { csvFirst.IsEnabled = csvLast.IsEnabled = csvRange.IsChecked; csvError.Text = ""; };
        }
        csvFirst.Keyboard = csvLast.Keyboard = Keyboard.Numeric;
        LocalizationBindings.Bind(csvFirst, SemanticProperties.DescriptionProperty, "FirstRowCountedFromRowOfTheFull");
        LocalizationBindings.Bind(csvLast, SemanticProperties.DescriptionProperty, "LastRowInclusive");
        var fields = new Grid { ColumnSpacing = 12, ColumnDefinitions = { new(GridLength.Star), new(GridLength.Star) } };
        var first = new VerticalStackLayout { Spacing = 4 };
        first.Add(LocalizedText("FirstRow", 12)); first.Add(csvFirst);
        var last = new VerticalStackLayout { Spacing = 4 };
        last.Add(LocalizedText("LastRowInclusive127", 12)); last.Add(csvLast);
        fields.Add(first); fields.Add(last, 1); panel.Add(fields);
        panel.Add(LocalizedText("UseRowNumbersFromTheCachedResultSQL", 12, color: Muted));
        panel.Add(csvError);
        var buttons = new HorizontalStackLayout { Spacing = 8 };
        var save = new Button { AutomationId = "CsvConfirm", BackgroundColor = Accent, TextColor = CanvasColor }.Localized(Button.TextProperty, "ChooseDestination").WithActionIcon();
        save.Clicked += (_, _) =>
        {
            try
            {
                var scope = csvRange.IsChecked ? CsvExportScope.Range : csvPage.IsChecked ? CsvExportScope.CurrentPage : CsvExportScope.All;
                long firstRow = 1, lastRow = 0;
                if (scope == CsvExportScope.Range &&
                    (!long.TryParse(csvFirst.Text?.Trim(), NumberStyles.None, CultureInfo.InvariantCulture, out firstRow) ||
                     !long.TryParse(csvLast.Text?.Trim(), NumberStyles.None, CultureInfo.InvariantCulture, out lastRow)))
                    throw new ArgumentException(L.Get("EnterWholeNumbersForTheFirstAndLast"));
                var range = CsvExportRange.Create(csvTotal, scope, csvCurrentPage, csvPageSize, firstRow, lastRow);
                csvOverlay.IsVisible = false; csvSelection?.TrySetResult(range);
            }
            catch (ArgumentException error) { csvError.Text = error.Message; }
        };
        var cancel = new Button { AutomationId = "CsvCancel" }.Localized(Button.TextProperty, "Cancel").WithActionIcon();
        cancel.Clicked += (_, _) => { csvOverlay.IsVisible = false; csvSelection?.TrySetResult(null); };
        buttons.Add(save); buttons.Add(cancel); panel.Add(buttons);
        csvOverlay.Add(new Border { Content = panel, HorizontalOptions = LayoutOptions.Center, VerticalOptions = LayoutOptions.Center });
    }

    private Task<CsvExportRange?> PickCsvRangeAsync(long total, long page, int pageSize)
    {
        csvTotal = total; csvCurrentPage = page; csvPageSize = pageSize;
        var current = CsvExportRange.Create(total, CsvExportScope.CurrentPage, page, pageSize);
        csvSummary.Text = total == 0 ? L.Get("TheResultContainsNoRowsOnlyColumnHeaders") : L.Get("ExportFromAllNCachedRows", total);
        csvAll.Content = L.Get("AllRowsN", total);
        csvPage.Content = current.Count == 0 ? L.Get("CurrentPageRows") :
            L.Get("CurrentPageRowsNNNRows", current.Start + 1, current.Start + current.Count, current.Count);
        csvFirst.Text = (current.Count == 0 ? 1 : current.Start + 1).ToString(CultureInfo.InvariantCulture);
        csvLast.Text = (current.Start + current.Count).ToString(CultureInfo.InvariantCulture);
        csvRange.IsEnabled = total > 0;
        csvAll.IsChecked = true; csvRange.IsChecked = csvPage.IsChecked = false;
        csvFirst.IsEnabled = csvLast.IsEnabled = false;
        csvError.Text = "";
        csvSelection = new(TaskCreationOptions.RunContinuationsAsynchronously);
        csvOverlay.IsVisible = true;
        return csvSelection.Task;
    }

    private async Task ExportAsync()
    {
        if (result.Columns.Count == 0) throw new InvalidOperationException(L.Get("ThereAreNoResultsToExport"));
        var loaded = loadedResult;
        var displayed = result;
        long total = loaded?.RowCount ?? displayed.Rows.Count;
        int pageSize = loaded is null ? Math.Max(1, displayed.Rows.Count) : LoadedQueryResult.PageSize;
        var range = await PickCsvRangeAsync(total, loaded is null ? 0 : resultPage, pageSize);
        if (range is null) return;
        var path = await PickPathAsync(L.Get("SaveCSVNRows", range.Count), Path.Combine(FileSystem.AppDataDirectory, "silverhamu-results.csv"), true);
        if (path is null) return;
        IEnumerable<SqlValue[]> rows = loaded is not null ? loaded.ReadRows(range.Start, range.Count) :
            displayed.Rows.Skip((int)range.Start).Take((int)range.Count);
        long written = await Task.Run(() => CsvExport.WriteAsync(path, displayed.Columns, rows));
        Log(L.Get("ExportedNOfNRowsToCSVRows", total, written, (written == 0 ? 0 : range.Start + 1), range.Start + written, path));
    }
}
