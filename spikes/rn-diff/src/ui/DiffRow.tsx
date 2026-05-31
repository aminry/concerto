import React from 'react';
import { Platform, Pressable, StyleSheet, Text, View } from 'react-native';

import type { Row } from '@/diff/types';
import { colors, MONO_FONT_SIZE, ROW_HEIGHT, syntaxColors } from './theme';

const MONO = Platform.select({ ios: 'Menlo', android: 'monospace', default: 'monospace' });

interface DiffRowProps {
  readonly row: Row;
  readonly collapsed: boolean;
  readonly onToggleFile: (fileIndex: number) => void;
  readonly onPressLine: (key: string) => void;
}

/**
 * One fixed-height row of the virtualized list. Memoized: the list re-renders
 * thousands of these on data change, so a stable reference per unchanged row
 * is what keeps scroll cheap. All three row kinds share the same height.
 */
function DiffRowImpl({ row, collapsed, onToggleFile, onPressLine }: DiffRowProps): React.ReactElement {
  if (row.type === 'file') {
    return (
      <Pressable
        style={styles.fileHeader}
        onPress={() => onToggleFile(row.fileIndex)}
        accessibilityRole="button"
        accessibilityLabel={`${collapsed ? 'Expand' : 'Collapse'} ${row.path}`}
      >
        <Text style={styles.chevron}>{collapsed ? '▸' : '▾'}</Text>
        <Text style={styles.filePath} numberOfLines={1} ellipsizeMode="head">
          {row.path}
        </Text>
        <Text style={styles.addCount}>+{row.addCount}</Text>
        <Text style={styles.delCount}>−{row.delCount}</Text>
      </Pressable>
    );
  }

  if (row.type === 'hunk') {
    return (
      <View style={styles.hunkHeader}>
        <Text style={styles.hunkText} numberOfLines={1}>
          {row.header}
          {row.section ? `  ${row.section}` : ''}
        </Text>
      </View>
    );
  }

  // line row
  const lineBg =
    row.kind === 'add' ? colors.addBg : row.kind === 'del' ? colors.delBg : undefined;
  const gutterBg =
    row.kind === 'add' ? colors.addGutter : row.kind === 'del' ? colors.delGutter : undefined;
  const sign = row.kind === 'add' ? '+' : row.kind === 'del' ? '−' : ' ';

  return (
    <Pressable
      style={[styles.lineRow, lineBg ? { backgroundColor: lineBg } : null]}
      onPress={() => onPressLine(row.key)}
    >
      <Text style={[styles.gutter, gutterBg ? { backgroundColor: gutterBg } : null]}>
        {row.oldNo ?? ''}
      </Text>
      <Text style={[styles.gutter, gutterBg ? { backgroundColor: gutterBg } : null]}>
        {row.newNo ?? ''}
      </Text>
      <Text style={styles.sign}>{sign}</Text>
      <Text style={styles.code} numberOfLines={1}>
        {row.tokens.map((t, i) => (
          <Text key={i} style={{ color: syntaxColors[t.cls] }}>
            {t.text}
          </Text>
        ))}
      </Text>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  fileHeader: {
    height: ROW_HEIGHT + 8,
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 8,
    backgroundColor: colors.panel,
    borderTopWidth: StyleSheet.hairlineWidth,
    borderColor: colors.border,
  },
  chevron: { color: colors.dim, width: 16, fontSize: MONO_FONT_SIZE },
  filePath: { color: colors.text, flex: 1, fontFamily: MONO, fontSize: MONO_FONT_SIZE },
  addCount: { color: colors.add, marginLeft: 8, fontSize: MONO_FONT_SIZE - 1 },
  delCount: { color: colors.del, marginLeft: 6, fontSize: MONO_FONT_SIZE - 1 },
  hunkHeader: {
    height: ROW_HEIGHT,
    justifyContent: 'center',
    paddingHorizontal: 8,
    backgroundColor: colors.hunkBg,
  },
  hunkText: { color: colors.hunkText, fontFamily: MONO, fontSize: MONO_FONT_SIZE - 1 },
  lineRow: {
    height: ROW_HEIGHT,
    flexDirection: 'row',
    alignItems: 'center',
    paddingRight: 8,
  },
  gutter: {
    width: 40,
    textAlign: 'right',
    paddingRight: 6,
    color: colors.gutter,
    fontFamily: MONO,
    fontSize: MONO_FONT_SIZE - 2,
    height: ROW_HEIGHT,
    lineHeight: ROW_HEIGHT,
  },
  sign: {
    width: 12,
    textAlign: 'center',
    color: colors.dim,
    fontFamily: MONO,
    fontSize: MONO_FONT_SIZE,
    lineHeight: ROW_HEIGHT,
  },
  code: {
    flex: 1,
    fontFamily: MONO,
    fontSize: MONO_FONT_SIZE,
    lineHeight: ROW_HEIGHT,
    paddingLeft: 4,
  },
});

export const DiffRow = React.memo(DiffRowImpl);
