import js from '@eslint/js'
import { defineConfig, globalIgnores } from 'eslint/config'
import reactHooks from 'eslint-plugin-react-hooks'
import tseslint from 'typescript-eslint'

export default defineConfig(
  globalIgnores(['dist', 'src/api/schema.d.ts']),
  js.configs.recommended,
  tseslint.configs.strict,
  reactHooks.configs.flat.recommended,
)
