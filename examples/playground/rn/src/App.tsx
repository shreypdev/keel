import { useCallback, useEffect, useRef, useState } from 'react';
import { ActivityIndicator, Pressable, StatusBar, StyleSheet, Text, View } from 'react-native';
import { SafeAreaProvider, SafeAreaView } from 'react-native-safe-area-context';
import { runBench } from './bench';
import { runChecks } from './checks';
import { BenchScreen, type BenchState } from './screens/BenchScreen';
import { BigListScreen } from './screens/BigListScreen';
import { TodosScreen } from './screens/TodosScreen';
import { colors } from './theme';
import { type Playground, describePanic, nativeCounters, startUndra } from './undra';

type Tab = 'todos' | 'biglist' | 'bench';
const TABS: ReadonlyArray<readonly [Tab, string]> = [
  ['todos', 'Todos'],
  ['biglist', '10k list'],
  ['bench', 'Bench'],
];

/** The to-dos the app starts with, added through the core so the first screen shows applied change-sets. */
const SEED = ['Put Undra under React Native', 'Run the 10k list', 'Read ADR-038'];

/**
 * The React Native playground: the playground's one Rust core, linked into the app and reached
 * through JSI by @undra/react-native, under the Todos and 10k-list screens of the web app, plus a
 * Bench screen with on-device self-checks and the measurements of ADR-038.
 */
export default function App() {
  const [playground, setPlayground] = useState<Playground | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>('todos');
  const [bench, setBench] = useState<BenchState>({ checks: null, bench: null, running: null, log: [], panic: null });
  const logLines = useRef<string[]>([]);

  const log = useCallback((line: string) => {
    console.log(line);
    logLines.current = [...logLines.current, line].slice(-200);
    setBench(state => ({ ...state, log: logLines.current }));
  }, []);

  const runAll = useCallback(
    async (p: Playground) => {
      setBench(state => ({ ...state, running: 'Running self-checks…' }));
      const checks = await runChecks(p, log);
      setBench(state => ({ ...state, checks, running: 'Measuring…' }));
      let rows = null;
      try {
        rows = await runBench(p.core, log);
      } catch (error) {
        log(`UNDRA-RN BENCH failed: ${error instanceof Error ? error.message : String(error)}`);
      }
      const counters = nativeCounters(p.core);
      log(`UNDRA-RN native counters ${JSON.stringify(counters)}`);
      log(`UNDRA-RN mirror ${JSON.stringify(p.core.mirror.stats())}`);
      setBench(state => ({ ...state, bench: rows, running: null }));
    },
    [log],
  );

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const p = await startUndra(log, report => setBench(state => ({ ...state, panic: describePanic(report) })));
        if (cancelled) {
          return;
        }
        for (const title of SEED) {
          await p.todos.add(title);
        }
        const first = p.todos.todos.get()[0];
        if (first !== undefined) {
          await p.todos.toggle(first.id);
        }
        log(
          `UNDRA-RN todos change-sets applied: todos=${p.todos.todos.get().length} visible=${p.todos.visible.get().length} remaining=${p.todos.remaining.get()} first.done=${String(p.todos.todos.get()[0]?.done)}`,
        );
        setPlayground(p);
        // The checks and measurements run once on their own, after the first screen has rendered.
        setTimeout(() => void runAll(p), 1500);
      } catch (error) {
        const message = error instanceof Error ? `${error.name}: ${error.message}` : String(error);
        log(`UNDRA-RN failed to start: ${message}`);
        setFailure(message);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [log, runAll]);

  return (
    <SafeAreaProvider>
      <StatusBar barStyle="dark-content" />
      <SafeAreaView style={styles.app} edges={['top', 'left', 'right']}>
        <View style={styles.header}>
          <Text style={styles.brand}>Undra playground</Text>
          <Text style={styles.sub}>React Native · native core over JSI</Text>
        </View>
        <View style={styles.body}>
          {failure !== null ? (
            <Text style={styles.failure}>{failure}</Text>
          ) : playground === null ? (
            <ActivityIndicator style={styles.loading} />
          ) : tab === 'todos' ? (
            <TodosScreen todos={playground.todos} />
          ) : tab === 'biglist' ? (
            <BigListScreen bigList={playground.bigList} />
          ) : (
            <BenchScreen state={bench} onRun={() => void runAll(playground)} />
          )}
        </View>
        <SafeAreaView edges={['bottom']} style={styles.tabs}>
          {TABS.map(([id, label]) => (
            <Pressable key={id} style={styles.tab} onPress={() => setTab(id)} testID={`tab-${id}`}>
              <Text style={[styles.tabText, tab === id && styles.tabOn]}>{label}</Text>
            </Pressable>
          ))}
        </SafeAreaView>
      </SafeAreaView>
    </SafeAreaProvider>
  );
}

const styles = StyleSheet.create({
  app: { flex: 1, backgroundColor: colors.background },
  header: { paddingHorizontal: 16, paddingTop: 8, paddingBottom: 6 },
  brand: { fontSize: 13, fontWeight: '700', color: colors.accent, letterSpacing: 0.5, textTransform: 'uppercase' },
  sub: { fontSize: 12, color: colors.muted },
  body: { flex: 1 },
  loading: { marginTop: 48 },
  failure: { color: colors.bad, padding: 16 },
  tabs: {
    flexDirection: 'row',
    borderTopWidth: StyleSheet.hairlineWidth,
    borderTopColor: colors.border,
    backgroundColor: colors.surface,
  },
  tab: { flex: 1, alignItems: 'center', paddingVertical: 12 },
  tabText: { color: colors.muted, fontWeight: '600' },
  tabOn: { color: colors.accent },
});
