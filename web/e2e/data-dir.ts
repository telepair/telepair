import path from 'path';

/** Absolute path to the dedicated e2e data dir, sibling of `e2e/`. */
export const E2E_DATA_DIR = path.resolve(import.meta.dirname, '..', '.e2e-data');
