import { useSignal } from '@undra/runtime/react';
import { type Filter, TodoError, type Todos } from '@playground/core';
import { useState } from 'react';
import { FlatList, Pressable, StyleSheet, Switch, Text, TextInput, View } from 'react-native';
import { colors, shared } from '../theme';

const FILTERS: readonly Filter[] = ['all', 'active', 'done'];

/**
 * The `Todos` store, as on the web: `visible` and `remaining` are computed in the core and arrive in
 * the same change-set as the write that changed them; this screen only reads them.
 */
export function TodosScreen({ todos }: { readonly todos: Todos }) {
  const visible = useSignal(todos.visible);
  const filter = useSignal(todos.filter);
  const remaining = useSignal(todos.remaining);
  const everything = useSignal(todos.todos);
  const [draft, setDraft] = useState('');
  const [problem, setProblem] = useState<string | null>(null);

  const add = async (): Promise<void> => {
    try {
      await todos.add(draft);
      setDraft('');
      setProblem(null);
    } catch (error) {
      // A refused add is a typed error: `TodoError.EmptyTitle`.
      setProblem(error instanceof TodoError ? error.message : String(error));
    }
  };

  return (
    <View style={shared.screen}>
      <View style={shared.titleRow}>
        <Text style={shared.title}>Todos</Text>
        <Text style={shared.badge} testID="remaining">
          {remaining} left
        </Text>
      </View>
      <View style={styles.inputRow}>
        <TextInput
          style={styles.input}
          value={draft}
          onChangeText={setDraft}
          onSubmitEditing={() => void add()}
          placeholder="What needs doing?"
          placeholderTextColor={colors.muted}
          returnKeyType="done"
          testID="todo-input"
        />
        <Pressable style={shared.primaryButton} onPress={() => void add()} testID="todo-add">
          <Text style={shared.primaryButtonText}>Add</Text>
        </Pressable>
      </View>
      {problem !== null && <Text style={shared.error}>{problem}</Text>}
      <View style={styles.filters}>
        {FILTERS.map(name => (
          <Pressable
            key={name}
            style={[shared.chip, filter === name && shared.chipOn]}
            onPress={() => void todos.setFilter(name)}
            testID={`filter-${name}`}>
            <Text style={[shared.chipText, filter === name && shared.chipTextOn]}>{name}</Text>
          </Pressable>
        ))}
        <View style={styles.spacer} />
        <Pressable
          style={[shared.chip, !everything.some(todo => todo.done) && styles.disabled]}
          disabled={!everything.some(todo => todo.done)}
          onPress={() => void todos.clearDone()}
          testID="clear-done">
          <Text style={shared.chipText}>Clear done</Text>
        </Pressable>
      </View>
      <FlatList
        data={visible}
        keyExtractor={todo => todo.id}
        ListEmptyComponent={
          <Text style={styles.empty}>
            {everything.length === 0 ? 'Nothing to do. Add something above.' : `No ${filter} items.`}
          </Text>
        }
        renderItem={({ item: todo }) => (
          <View style={styles.item} testID="todo-item">
            <Switch value={todo.done} onValueChange={() => void todos.toggle(todo.id)} />
            <Text style={[styles.itemText, todo.done && styles.done]}>{todo.title}</Text>
            <Pressable onPress={() => void todos.remove(todo.id)} hitSlop={12}>
              <Text style={styles.remove}>×</Text>
            </Pressable>
          </View>
        )}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  inputRow: { flexDirection: 'row', gap: 8, marginBottom: 8 },
  input: {
    flex: 1,
    borderWidth: 1,
    borderColor: colors.border,
    borderRadius: 8,
    paddingHorizontal: 12,
    paddingVertical: 10,
    color: colors.text,
    backgroundColor: colors.surface,
  },
  filters: { flexDirection: 'row', alignItems: 'center', gap: 6, marginVertical: 8 },
  spacer: { flex: 1 },
  disabled: { opacity: 0.4 },
  empty: { color: colors.muted, textAlign: 'center', marginTop: 24 },
  item: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
    paddingVertical: 10,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: colors.border,
  },
  itemText: { flex: 1, fontSize: 16, color: colors.text },
  done: { color: colors.muted, textDecorationLine: 'line-through' },
  remove: { fontSize: 22, color: colors.muted, paddingHorizontal: 4 },
});
