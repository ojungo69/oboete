import eslint from '@eslint/js';
import tseslint from 'typescript-eslint';

export default tseslint.config(
  {
    ignores: [
      'node_modules/**',
      'dist/**',
      'build/**',
      'coverage/**',
      'legacy/**',
      '.specify/**',
      'scripts/e2e/**',
      '.tmp/**',
    ],
  },
  {
    files: ['src/**/*.ts', 'src/launcher.mjs', 'test/**/*.ts', 'scripts/build.mjs', 'scripts/quality-debt-*.mjs', 'eslint.config.js'],
    extends: [eslint.configs.recommended, ...tseslint.configs.recommended],
    rules: {
      '@typescript-eslint/consistent-type-imports': 'error',
    },
  },
  {
    files: ['src/viewer/app/**/*.ts', 'src/viewer/app/**/*.tsx'],
    extends: [eslint.configs.recommended, ...tseslint.configs.recommended],
    languageOptions: {
      globals: {
        document: 'readonly',
        fetch: 'readonly',
        location: 'readonly',
        EventSource: 'readonly',
        HTMLElement: 'readonly',
        HTMLInputElement: 'readonly',
        RequestInit: 'readonly',
        URLSearchParams: 'readonly',
      },
    },
  },
  {
    files: ['src/launcher.mjs', 'scripts/build.mjs', 'scripts/quality-debt-*.mjs', 'eslint.config.js'],
    languageOptions: {
      sourceType: 'module',
      globals: {
        console: 'readonly',
        process: 'readonly',
        fetch: 'readonly',
        URL: 'readonly',
        URLSearchParams: 'readonly',
      },
    },
  },
);
