import { useSignal } from '@undra/runtime/react';
import { type BigList, type Item, ListError } from '@playground/core';
import { useCallback, useEffect, useRef, useState } from 'react';
import { FlatList, Pressable, StyleSheet, Switch, Text, View, type ListViewToken } from 'react-native';
import { colors, shared } from '../theme';

/** Every row is this tall, so FlatList knows where row N is without measuring (10,000 rows). */
const ROW_HEIGHT = 44;
/** How often the stream updates a visible row: ten times a second. */
const STREAM_INTERVAL_MS = 100;

/**
 * The `BigList` store: 10,000 keyed rows. Every button is a one-row operation that reaches this
 * screen as a keyed patch of one operation (the core never sends the list again).
 */
export function BigListScreen({ bigList }: { readonly bigList: BigList }) {
  const items = useSignal(bigList.items);
  const count = useSignal(bigList.count);
  const [firstVisible, setFirstVisible] = useState(0);
  const [streaming, setStreaming] = useState(false);
  const [readout, setReadout] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  const visibleWindow = useRef({ firstVisible, total: items.length });
  useEffect(() => {
    visibleWindow.current = { firstVisible, total: items.length };
  });

  useEffect(() => {
    if (!streaming) {
      return;
    }
    let tick = 0;
    const timer = setInterval(() => {
      const { firstVisible: first, total } = visibleWindow.current;
      if (total === 0) {
        return;
      }
      const index = first + Math.floor(Math.random() * Math.min(10, total - first));
      bigList.updateAt(index, `Streamed update ${++tick}`).catch(() => {});
    }, STREAM_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [streaming, bigList]);

  /** One operation, timed: the call through JSI, the core, the change-set and its application. */
  const operate = async (name: string, run: () => Promise<unknown>): Promise<void> => {
    const started = performance.now();
    try {
      await run();
      const ms = performance.now() - started;
      setReadout(`${name}: ${ms.toFixed(2)} ms`);
      setProblem(null);
      console.log(`UNDRA-RN biglist ${name} applied in ${ms.toFixed(2)} ms; rows=${bigList.items.get().length}`);
    } catch (error) {
      setProblem(error instanceof ListError ? error.message : String(error));
    }
  };

  const onViewable = useRef(({ viewableItems }: { viewableItems: ListViewToken[] }) => {
    const first = viewableItems[0]?.index;
    if (typeof first === 'number') {
      setFirstVisible(first);
    }
  }).current;
  const getItemLayout = useCallback(
    (_data: ArrayLike<Item> | null | undefined, index: number) => ({ length: ROW_HEIGHT, offset: ROW_HEIGHT * index, index }),
    [],
  );

  const middle = Math.floor(items.length / 2);
  const buttons: ReadonlyArray<readonly [string, string, () => Promise<unknown>]> = [
    ['Insert top', 'insert at top', () => bigList.insertAt(0, 'Inserted at the top')],
    ['Insert middle', 'insert in the middle', () => bigList.insertAt(middle, 'Inserted in the middle')],
    ['Update', 'update', () => bigList.updateAt(firstVisible, `Updated at ${new Date().toLocaleTimeString()}`)],
    ['Move', 'move to the middle', () => bigList.moveItem(firstVisible, middle)],
    ['Remove', 'remove', () => bigList.removeAt(firstVisible)],
    ['Reset', 'reset', () => bigList.reset()],
  ];

  return (
    <View style={shared.screen}>
      <View style={shared.titleRow}>
        <Text style={shared.title}>10k list</Text>
        <Text style={shared.badge} testID="biglist-count">
          {count.toLocaleString()}
        </Text>
      </View>
      <View style={styles.buttons}>
        {buttons.map(([label, name, run]) => (
          <Pressable key={label} style={shared.chip} onPress={() => void operate(name, run)} testID={`biglist-${name}`}>
            <Text style={shared.chipText}>{label}</Text>
          </Pressable>
        ))}
        <View style={styles.stream}>
          <Switch value={streaming} onValueChange={setStreaming} testID="biglist-stream" />
          <Text style={styles.note}>Stream</Text>
        </View>
      </View>
      <Text style={styles.note} testID="biglist-timing">
        {readout ?? 'Update, Move and Remove act on the first visible row.'}
      </Text>
      {problem !== null && <Text style={shared.error}>{problem}</Text>}
      <FlatList
        data={items}
        keyExtractor={item => String(item.id)}
        getItemLayout={getItemLayout}
        onViewableItemsChanged={onViewable}
        initialNumToRender={20}
        windowSize={7}
        renderItem={({ item, index }) => (
          <View style={styles.row} testID="biglist-row">
            <Text style={styles.rowIndex}>{index + 1}</Text>
            <Text style={styles.rowId}>#{item.id}</Text>
            <Text style={styles.rowLabel} numberOfLines={1}>
              {item.label}
            </Text>
            <Text style={[styles.rowVersion, item.version > 0 && styles.rowVersionHot]}>v{item.version}</Text>
          </View>
        )}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  buttons: { flexDirection: 'row', flexWrap: 'wrap', gap: 6, alignItems: 'center' },
  stream: { flexDirection: 'row', alignItems: 'center', gap: 6, marginLeft: 4 },
  note: { color: colors.muted, fontSize: 13, marginVertical: 6 },
  row: {
    height: ROW_HEIGHT,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: colors.border,
  },
  rowIndex: { width: 48, color: colors.muted, fontVariant: ['tabular-nums'], fontSize: 12 },
  rowId: { width: 64, color: colors.muted, fontVariant: ['tabular-nums'] },
  rowLabel: { flex: 1, color: colors.text },
  rowVersion: { color: colors.muted, fontVariant: ['tabular-nums'] },
  rowVersionHot: { color: colors.accent, fontWeight: '600' },
});
