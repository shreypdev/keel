import { useSignal } from '@undra/runtime/react';
import type { Notes } from '@playground/core';
import { useEffect, useState } from 'react';
import { FlatList, Pressable, StyleSheet, Switch, Text, TextInput, View } from 'react-native';
import { colors, shared } from '../theme';

/** The database this screen keeps its notes in (the checks use their own). */
const DATABASE = 'notes';

/**
 * The `Notes` store: a list mirrored from a SQLite table through the native `Db` port (ADR-048). Every change goes
 * to the database first (on the module's own thread, off the JS thread); the list follows once it is written, and it
 * is still there after the app is killed.
 */
export function NotesScreen({ notes }: { readonly notes: Notes }) {
  const list = useSignal(notes.notes);
  const version = useSignal(notes.version);
  const [draft, setDraft] = useState('');
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    notes.open(DATABASE).then(
      () => setProblem(null),
      (error: unknown) => setProblem(String(error)),
    );
  }, [notes]);

  const attempt = (work: () => Promise<unknown>): void => {
    work().then(
      () => setProblem(null),
      (error: unknown) => setProblem(String(error)),
    );
  };

  const add = (): void => {
    const title = draft.trim();
    if (title === '') return;
    attempt(async () => {
      await notes.add(title);
      setDraft('');
    });
  };

  return (
    <View style={shared.screen}>
      <View style={shared.titleRow}>
        <Text style={shared.title}>Notes</Text>
        <Text style={shared.badge} testID="notes-version">
          SQLite · schema v{version}
        </Text>
      </View>
      <View style={styles.inputRow}>
        <TextInput
          style={styles.input}
          value={draft}
          onChangeText={setDraft}
          onSubmitEditing={add}
          placeholder="Write a note"
          placeholderTextColor={colors.muted}
          returnKeyType="done"
          testID="note-input"
        />
        <Pressable style={shared.primaryButton} onPress={add} testID="note-add">
          <Text style={shared.primaryButtonText}>Add</Text>
        </Pressable>
      </View>
      {problem !== null && <Text style={shared.error}>{problem}</Text>}
      <FlatList
        data={list}
        keyExtractor={note => String(note.id)}
        ListEmptyComponent={<Text style={styles.empty}>No notes yet. They are kept in a SQLite database on this device.</Text>}
        renderItem={({ item: note }) => (
          <View style={styles.item} testID="note-item">
            <Switch value={note.done} onValueChange={() => attempt(() => notes.toggle(note.id))} />
            <Text style={[styles.itemText, note.done && styles.done]}>{note.title}</Text>
            <Pressable onPress={() => attempt(() => notes.remove(note.id))} hitSlop={12}>
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
