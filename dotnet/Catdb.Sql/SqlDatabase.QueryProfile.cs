using System.Diagnostics;

namespace Catdb.Sql;

public sealed partial class SqlDatabase
{
    private sealed class StepMeasurement(PlanStep step)
    {
        public PlanStep Step { get; } = step;
        public long Rows { get; set; }
        public long Loops { get; set; }
        public long ElapsedTicks { get; set; }
    }

    private sealed class QueryProfile
    {
        private sealed class Scope
        {
            public Dictionary<object, QueryProfile> Sites { get; } = new(ReferenceEqualityComparer.Instance);
            public Dictionary<string, QueryProfile> Tags { get; } = new(StringComparer.Ordinal);
            public Dictionary<PlanStep, StepMeasurement> Steps { get; } = [];
        }

        private readonly List<StepMeasurement> ordered;
        private readonly Scope scope;
        private QueryProfile? planning;
        public bool IsPlanning { get; }
        public IReadOnlyList<StepMeasurement> Measurements => ordered;

        public QueryProfile() : this([], new Scope(), false) { }
        private QueryProfile(List<StepMeasurement> ordered, Scope scope, bool planning) =>
            (this.ordered, this.scope, IsPlanning) = (ordered, scope, planning);

        public QueryProfile Planning() => IsPlanning ? this : planning ??= new(ordered, scope, true);
        public QueryProfile Child(object site) => GetChild(scope.Sites, site);
        public QueryProfile Child(string tag) => GetChild(scope.Tags, tag);

        private QueryProfile GetChild<T>(Dictionary<T, QueryProfile> children, T key) where T : notnull
        {
            if (!children.TryGetValue(key, out var child))
                children.Add(key, child = new(ordered, new Scope(), false));
            return IsPlanning ? child.Planning() : child;
        }

        public long Start() => IsPlanning ? 0 : Stopwatch.GetTimestamp();
        public void Declare(PlanStep step) => Measurement(step);

        public void Record(PlanStep step, long rows, long started) =>
            RecordElapsed(step, rows, IsPlanning ? 0 : Stopwatch.GetTimestamp() - started);

        public void RecordElapsed(PlanStep step, long rows, long elapsedStopwatchTicks)
        {
            var measure = Measurement(step);
            if (IsPlanning) return;
            measure.Loops++;
            measure.Rows += rows;
            measure.ElapsedTicks += elapsedStopwatchTicks;
        }

        private StepMeasurement Measurement(PlanStep step)
        {
            if (scope.Steps.TryGetValue(step, out var measure)) return measure;
            scope.Steps.Add(step, measure = new(step));
            ordered.Add(measure);
            return measure;
        }
    }
}
