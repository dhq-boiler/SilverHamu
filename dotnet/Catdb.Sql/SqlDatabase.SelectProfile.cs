using System.Diagnostics;

namespace Catdb.Sql;

public sealed partial record SelectRows
{
    internal SelectRows? ProfileSource { get; init; }
}

public sealed partial class SqlDatabase
{
    private sealed class OperatorTiming
    {
        private readonly QueryProfile profile;
        private readonly PlanStep step;
        private long elapsed;
        private long rows;
        private bool ran;

        internal OperatorTiming(QueryProfile profile, PlanStep step)
        {
            this.profile = profile;
            this.step = step;
            profile.Declare(step);
        }

        internal long Start() => profile.Start();

        internal void End(long start, long produced)
        {
            if (profile.IsPlanning) return;
            elapsed += Stopwatch.GetTimestamp() - start;
            rows += produced;
            ran = true;
        }

        internal IEnumerable<T> Observe<T>(IEnumerable<T> source)
        {
            var opened = Start();
            using var iterator = source.GetEnumerator();
            End(opened, 0);
            while (true)
            {
                var start = Start();
                var next = iterator.MoveNext();
                End(start, next ? 1 : 0);
                if (!next) yield break;
                yield return iterator.Current;
            }
        }

        internal void Publish()
        {
            if (ran) profile.RecordElapsed(step, rows, elapsed);
        }
    }

    private sealed class SelectTimings : IDisposable
    {
        private readonly List<OperatorTiming> stages = [];
        internal OperatorTiming? Where { get; }
        internal OperatorTiming? Groups { get; }
        internal OperatorTiming? Having { get; }
        internal OperatorTiming? Project { get; }
        internal OperatorTiming? Sort { get; }
        internal OperatorTiming? Distinct { get; }
        internal OperatorTiming? Page { get; }
        internal OperatorTiming? Combine { get; }
        internal Dictionary<int, OperatorTiming> Windows { get; } = [];

        internal SelectTimings(QueryProfile profile, SelectRows bound)
        {
            var query = bound.ProfileSource ?? bound;
            OperatorTiming Stage(string op, string? detail = null, QueryProfile? site = null)
            {
                var timing = new OperatorTiming(site ?? profile, new PlanStep(op, null, detail));
                stages.Add(timing);
                return timing;
            }

            if (query.SetExpression is { } set) Combine = Stage(set.Operation, "Combine query results");
            else
            {
                if (query.Where is not null) Where = Stage("FILTER", "WHERE");
                if (bound.IsGrouped)
                {
                    Groups = Stage("AGGREGATE", query.GroupBy is { } keys
                        ? string.Join(", ", keys.Select(key => key.Name)) : "Single group");
                    if (query.Having is not null) Having = Stage("FILTER", "HAVING");
                    Project = Stage("PROJECT");
                }
                for (int i = 0; i < bound.Projections.Count; i++)
                    if (bound.Projections[i].Window is { } window)
                        Windows[i] = Stage("WINDOW", window.Function, profile.Child($"window:{i}"));
            }
            if (query.Order.Count > 0)
                Sort = Stage("SORT", string.Join(", ", query.Order.Select(order => $"{order.Column} {(order.Descending ? "DESC" : "ASC")}")));
            if (Combine is null && !bound.IsGrouped && query.Distinct) Project = Stage("PROJECT");
            if (query.Distinct) Distinct = Stage("DISTINCT", "Remove duplicate projected rows");
            if (query.Offset > 0 || query.Limit.HasValue) Page = Stage("LIMIT", $"Offset {query.Offset}, limit {query.Limit ?? MaxRows}");
            if (Combine is null && !bound.IsGrouped && !query.Distinct) Project = Stage("PROJECT");
        }

        public void Dispose()
        {
            foreach (var stage in stages) stage.Publish();
        }
    }
}
