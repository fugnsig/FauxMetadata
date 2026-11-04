import js from '@eslint/js';
import ts from 'typescript-eslint';
export default ts.config(js.configs.recommended, ...ts.configs.recommended, {
  files: ['**/*.ts'],
  languageOptions: { globals: { document: 'readonly', crypto: 'readonly', console: 'readonly' } },
});
