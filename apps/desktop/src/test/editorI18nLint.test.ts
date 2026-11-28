import fs from 'node:fs';
import path from 'node:path';
import { enTranslations, idTranslations, t } from '@clipper-helper/i18n';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';

const COMPONENTS_DIR = path.resolve(__dirname, '../components');

/**
 * Pure punctuation, icons, whitespace, and technical units that are intentionally
 * language-neutral in JSX text nodes (e.g. `←`, `↶`, `·`, `fps`, `×`).
 */
const ALLOWED_JSX_TEXT = new Set([
  '',
  ':',
  '·',
  '×',
  'fps',
  '←',
  '→',
  '↶',
  '↷',
  '⏮',
  '⏭',
  '●',
]);

const USER_VISIBLE_ATTRS = new Set(['aria-label', 'placeholder', 'title', 'alt']);

function collectEditorComponentFiles(): string[] {
  const entries = fs.readdirSync(COMPONENTS_DIR);
  return entries
    .filter((name) => /^Editor.*\.tsx$/.test(name))
    .sort()
    .map((name) => path.join(COMPONENTS_DIR, name));
}

function findHardcodedUiStrings(filePath: string): string[] {
  const sourceText = fs.readFileSync(filePath, 'utf8');
  const sf = ts.createSourceFile(filePath, sourceText, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const violations: string[] = [];
  const fileName = path.basename(filePath);

  function visit(node: ts.Node): void {
    if (ts.isJsxText(node)) {
      const normalized = node.getText(sf).replace(/\s+/g, ' ').trim();
      if (normalized && !ALLOWED_JSX_TEXT.has(normalized) && /[\p{L}\p{N}]/u.test(normalized)) {
        const { line } = sf.getLineAndCharacterOfPosition(node.getStart(sf));
        violations.push(`${fileName}:${line + 1} JSX text: ${JSON.stringify(normalized)}`);
      }
    } else if (ts.isJsxAttribute(node)) {
      const attrName = node.name.getText(sf);
      if (USER_VISIBLE_ATTRS.has(attrName) && node.initializer) {
        if (ts.isStringLiteral(node.initializer)) {
          const val = node.initializer.text.trim();
          // Decorative empty alt="" is allowed
          if (!(attrName === 'alt' && val === '')) {
            const { line } = sf.getLineAndCharacterOfPosition(node.getStart(sf));
            violations.push(`${fileName}:${line + 1} attribute ${attrName}=${JSON.stringify(val)}`);
          }
        }
      }
    }
    ts.forEachChild(node, visit);
  }

  visit(sf);
  return violations;
}

function flattenKeys(obj: Record<string, unknown>, prefix = ''): string[] {
  const keys: string[] = [];
  for (const [k, v] of Object.entries(obj)) {
    const full = prefix ? `${prefix}.${k}` : k;
    if (v && typeof v === 'object' && !Array.isArray(v)) {
      keys.push(...flattenKeys(v as Record<string, unknown>, full));
    } else {
      keys.push(full);
    }
  }
  return keys;
}

describe('Editor i18n audit & hard-coded string lint', () => {
  it('has zero hard-coded user-visible strings in Editor components', () => {
    const files = collectEditorComponentFiles();
    expect(files.length).toBeGreaterThanOrEqual(2);

    const allViolations = files.flatMap(findHardcodedUiStrings);
    expect(allViolations).toEqual([]);
  });

  it('catches hard-coded JSX text & attributes in a synthetic snippet', () => {
    const tmpSf = ts.createSourceFile(
      'Synthetic.tsx',
      `export function Demo() { return <button aria-label="Save">Back to Home</button>; }`,
      ts.ScriptTarget.Latest,
      true,
      ts.ScriptKind.TSX
    );
    const hits: string[] = [];
    const scan = (n: ts.Node) => {
      if (ts.isJsxText(n) && n.getText(tmpSf).trim()) hits.push(n.getText(tmpSf).trim());
      if (ts.isJsxAttribute(n) && USER_VISIBLE_ATTRS.has(n.name.getText(tmpSf))) {
        hits.push(n.name.getText(tmpSf));
      }
      ts.forEachChild(n, scan);
    };
    scan(tmpSf);
    expect(hits).toEqual(['aria-label', 'Back to Home']);
  });

  it('provides Indonesian (default) and English translations for every editor.* key', () => {
    const idEditor = (idTranslations as { editor: Record<string, unknown> }).editor;
    const enEditor = (enTranslations as { editor: Record<string, unknown> }).editor;

    const idKeys = flattenKeys(idEditor, 'editor').sort();
    const enKeys = flattenKeys(enEditor, 'editor').sort();
    expect(idKeys).toEqual(enKeys);
    expect(idKeys.length).toBeGreaterThanOrEqual(45);

    for (const key of idKeys) {
      const idVal = t(key, undefined, 'id');
      const enVal = t(key, undefined, 'en');
      expect(idVal).not.toBe(key);
      expect(enVal).not.toBe(key);
      expect(idVal.trim().length).toBeGreaterThan(0);
      expect(enVal.trim().length).toBeGreaterThan(0);
    }
  });
});
