// Shared helpers for the generated bindings.

/** Convert a camelCase identifier to snake_case (`addNumbers` -> `add_numbers`). */
export function camelToSnake(name: string): string {
  return name.replace(/[A-Z]/g, (c) => `_${c.toLowerCase()}`);
}
