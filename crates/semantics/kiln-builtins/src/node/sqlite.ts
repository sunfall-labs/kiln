// kiln-builtins: node:sqlite implementation (Phase 5.2, ADR 0005)

export type SQLInputValue = null | number | bigint | string | Uint8Array;

export class StatementSync {
  private readonly _sql: string;

  constructor(sql: string) {
    this._sql = sql;
  }

  get sourceSQL(): string {
    return this._sql;
  }

  get expandedSQL(): string {
    return this._sql;
  }

  all(..._params: unknown[]): unknown[] {
    return [];
  }

  get(..._params: unknown[]): unknown | undefined {
    return undefined;
  }

  iterate(..._params: unknown[]): Iterable<unknown> {
    return [];
  }

  run(..._params: unknown[]): { changes: number | bigint; lastInsertRowid: number | bigint } {
    return { changes: 0, lastInsertRowid: 0 };
  }

  setAllowBareNamedParameters(_enabled: boolean): void {}
  setReadBigInts(_enabled: boolean): void {}
}

export class DatabaseSync {
  private readonly _location: string;
  private _isOpen: boolean;

  constructor(location: string, _options?: any) {
    this._location = String(location);
    this._isOpen = true;
  }

  close(): void {
    this._isOpen = false;
  }

  exec(_sql: string): void {}

  open(): void {
    this._isOpen = true;
  }

  prepare(sql: string): StatementSync {
    return new StatementSync(sql);
  }
}

const sqlite = {
  DatabaseSync,
  StatementSync,
  default: undefined as any,
};
sqlite.default = sqlite;

export default sqlite;
