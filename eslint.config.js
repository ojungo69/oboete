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
    files: ['src/**/*.ts', 'test/**/*.ts', 'scripts/build.mjs', 'eslint.config.js'],
    extends: [eslint.configs.recommended, ...tseslint.configs.recommended],
    rules: {
      '@typescript-eslint/consistent-type-imports': 'error',
    },
  },
  {
    files: ['scripts/build.mjs', 'eslint.config.js'],
    languageOptions: {
      sourceType: 'module',
      globals: {
        console: 'readonly',
        process: 'readonly',
      },
    },
  },
);
