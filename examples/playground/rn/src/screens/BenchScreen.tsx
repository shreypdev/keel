import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';
import type { BenchRow } from '../bench';
import type { CheckResult } from '../checks';
import { colors, shared } from '../theme';

/** What the Bench screen shows: the self-checks, the measurements and the evidence log. */
export interface BenchState {
  readonly checks: readonly CheckResult[] | null;
  readonly bench: readonly BenchRow[] | null;
  readonly running: string | null;
  readonly log: readonly string[];
  /** The last panic report the core handed to `onPanic` (ADR-046), as lines; `null` before the first. */
  readonly panic: string | null;
}

/** Self-checks of the boundary on this device, the measurements of ADR-038, and the log. */
export function BenchScreen({ state, onRun }: { readonly state: BenchState; readonly onRun: () => void }) {
  const passed = state.checks?.filter(c => c.pass).length ?? 0;
  return (
    <ScrollView style={shared.screen} contentContainerStyle={styles.content}>
      <View style={shared.titleRow}>
        <Text style={shared.title}>Bench</Text>
        {state.checks !== null && (
          <Text style={[shared.badge, passed === state.checks.length ? styles.ok : styles.bad]} testID="checks-badge">
            {passed}/{state.checks.length} checks
          </Text>
        )}
      </View>
      <Pressable
        style={[shared.primaryButton, state.running !== null && styles.disabled]}
        disabled={state.running !== null}
        onPress={onRun}
        testID="bench-run">
        <Text style={shared.primaryButtonText}>{state.running ?? 'Run checks and benchmarks again'}</Text>
      </Pressable>
      <Text style={styles.section}>Self-checks against the native core</Text>
      {(state.checks ?? []).map(check => (
        <View key={check.id} style={styles.line}>
          <Text style={[styles.mark, check.pass ? styles.ok : styles.bad]}>{check.pass ? '✓' : '✗'}</Text>
          <View style={styles.flex}>
            <Text style={styles.name}>
              {check.id} {check.title}
            </Text>
            <Text style={styles.detail}>{check.detail}</Text>
          </View>
        </View>
      ))}
      <Text style={styles.section}>Measurements</Text>
      {(state.bench ?? []).map(row => (
        <View key={row.name} style={styles.line}>
          <Text style={styles.value}>
            {row.value.toFixed(row.unit === 'ns' ? 0 : 2)} {row.unit}
          </Text>
          <View style={styles.flex}>
            <Text style={styles.name}>{row.name}</Text>
            <Text style={styles.detail}>{row.note}</Text>
          </View>
        </View>
      ))}
      <Text style={styles.section}>Last panic report (onPanic)</Text>
      <Text style={styles.log} testID="last-panic">
        {state.panic ?? 'No panic yet.'}
      </Text>
      <Text style={styles.section}>Log</Text>
      {state.log.map((line, index) => (
        <Text key={index} style={styles.log}>
          {line}
        </Text>
      ))}
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  content: { paddingBottom: 48 },
  section: { marginTop: 18, marginBottom: 6, fontWeight: '700', color: colors.text, fontSize: 15 },
  line: { flexDirection: 'row', gap: 10, paddingVertical: 6, alignItems: 'flex-start' },
  mark: { width: 18, fontSize: 16, fontWeight: '700' },
  ok: { color: colors.ok },
  bad: { color: colors.bad },
  flex: { flex: 1 },
  name: { color: colors.text, fontWeight: '600' },
  detail: { color: colors.muted, fontSize: 12 },
  value: { width: 92, color: colors.accent, fontWeight: '700', fontVariant: ['tabular-nums'] },
  log: { color: colors.muted, fontSize: 11, fontFamily: 'Menlo' },
  disabled: { opacity: 0.5 },
});
