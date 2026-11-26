import js from '@eslint/js';
import globals from 'globals';
import tseslint from 'typescript-eslint';

export default tseslint.config(
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    ignores: [
      '**/dist/**',
      '**/target/**',
      '**/node_modules/**',
      '**/spikes/**',
      'apps/bridge/**',
      'apps/desktop/src/ipc/bindings.ts',
    ],
  },
  {
    files: ['tools/**/*.{js,ts}', '*.{js,mjs}'],
    languageOptions: {
      globals: {
        ...globals.node,
      },
    },
  },
  {
    rules: {
      '@typescript-eslint/no-unused-vars': [
        'error',
        { argsIgnorePattern: '^_', varsIgnorePattern: '^_' },
      ],
      '@typescript-eslint/no-explicit-any': 'error',
    },
  }
);
