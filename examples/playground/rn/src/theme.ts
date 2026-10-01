import { StyleSheet } from 'react-native';

/** The playground's palette (light). */
export const colors = {
  background: '#f6f7f9',
  surface: '#ffffff',
  text: '#16181d',
  muted: '#6b7280',
  border: '#d9dde3',
  accent: '#4f46e5',
  ok: '#15803d',
  bad: '#b91c1c',
};

/** Styles the screens share. */
export const shared = StyleSheet.create({
  screen: { flex: 1, paddingHorizontal: 16, paddingTop: 8, backgroundColor: colors.background },
  titleRow: { flexDirection: 'row', alignItems: 'center', gap: 10, marginBottom: 10 },
  title: { fontSize: 24, fontWeight: '700', color: colors.text },
  badge: {
    paddingHorizontal: 10,
    paddingVertical: 3,
    borderRadius: 999,
    overflow: 'hidden',
    backgroundColor: '#e0e7ff',
    color: colors.accent,
    fontWeight: '600',
  },
  primaryButton: {
    backgroundColor: colors.accent,
    borderRadius: 8,
    paddingHorizontal: 16,
    paddingVertical: 10,
    alignItems: 'center',
    justifyContent: 'center',
  },
  primaryButtonText: { color: '#ffffff', fontWeight: '600' },
  chip: {
    borderWidth: 1,
    borderColor: colors.border,
    borderRadius: 999,
    paddingHorizontal: 12,
    paddingVertical: 6,
    backgroundColor: colors.surface,
  },
  chipOn: { backgroundColor: colors.accent, borderColor: colors.accent },
  chipText: { color: colors.text, fontWeight: '500' },
  chipTextOn: { color: '#ffffff' },
  error: { color: colors.bad, marginVertical: 4 },
});
