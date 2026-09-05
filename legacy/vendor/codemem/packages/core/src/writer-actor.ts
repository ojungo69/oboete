import { appendFileSync, mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import BetterSqlite3 from "better-sqlite3";

type RawDatabase = import("better-sqlite3").Database;
type RegistrationOptions = import("better-sqlite3").RegistrationOptions;
type BackupOptions = import("better-sqlite3").BackupOptions;
type BackupMetadata = import("better-sqlite3").BackupMetadata;
type SerializeOptions = import("better-sqlite3").SerializeOptions;
// biome-ignore lint/suspicious/noExplicitAny: Mirrors better-sqlite3's callback contract.
type VariableArgFunction = (...params: any[]) => unknown;
type ArgumentTypes<F extends VariableArgFunction> = F extends (...args: infer A) => unknown
	? A
	: never;
// biome-ignore lint/suspicious/noExplicitAny: Mirrors better-sqlite3's scalar function contract.
type SqliteFunctionCallback = (...params: any[]) => any;
type RawStatement<BindParameters extends unknown[], Result> = import("better-sqlite3").Statement<
	BindParameters,
	Result
>;
// biome-ignore lint/complexity/noBannedTypes: Mirrors better-sqlite3's named-bind generic.
type NamedBindParameters = {};
type PrepareBindParameters = unknown[] | NamedBindParameters;
type StatementBindParameters<BindParameters extends PrepareBindParameters> =
	BindParameters extends unknown[] ? BindParameters : [BindParameters];
type AuditedPreparedStatement<
	BindParameters extends PrepareBindParameters,
	Result,
> = BindParameters extends unknown[]
	? AuditedStatement<BindParameters, Result>
	: AuditedStatement<[BindParameters], Result>;
type AuditedTransaction<F extends VariableArgFunction> = import("better-sqlite3").Transaction<F> & {
	readonly database: AuditedSqliteConnection;
};

const DB_OPEN_TRACE_ENV = "CODEMEM_DB_OPEN_TRACE";

function recordOpen(dbPath: string, mode: "writer" | "readonly", owner: string): void {
	const tracePath = process.env[DB_OPEN_TRACE_ENV]?.trim();
	if (!tracePath) return;
	appendFileSync(
		tracePath,
		`${JSON.stringify({
			version: 1,
			event: "sqlite_open",
			mode,
			owner,
			pid: process.pid,
			dbPath: resolve(dbPath),
			openedAt: new Date().toISOString(),
		})}\n`,
		{ encoding: "utf8", mode: 0o600 },
	);
}

/**
 * Audited capability around a SQLite connection.
 *
 * The raw better-sqlite3 object is an ECMAScript private field so callers
 * cannot obtain it through the public package surface. Fluent methods return
 * the audited capability rather than leaking the wrapped connection.
 */
class AuditedSqliteConnection implements RawDatabase {
	readonly #raw: RawDatabase;

	protected constructor(raw: RawDatabase) {
		this.#raw = raw;
	}

	prepare<BindParameters extends PrepareBindParameters = unknown[], Result = unknown>(
		source: string,
	): AuditedPreparedStatement<BindParameters, Result> {
		const raw = this.#raw.prepare<BindParameters, Result>(source) as RawStatement<
			StatementBindParameters<BindParameters>,
			Result
		>;
		return new AuditedStatement(this, raw) as AuditedPreparedStatement<BindParameters, Result>;
	}

	get memory(): boolean {
		return this.#raw.memory;
	}

	get readonly(): boolean {
		return this.#raw.readonly;
	}

	get name(): string {
		return this.#raw.name;
	}

	get open(): boolean {
		return this.#raw.open;
	}

	get inTransaction(): boolean {
		return this.#raw.inTransaction;
	}

	transaction<F extends VariableArgFunction>(fn: F): AuditedTransaction<F> {
		const raw = this.#raw.transaction(fn);
		const audited = Object.assign((...params: ArgumentTypes<F>) => raw(...params), {
			default: (...params: ArgumentTypes<F>) => raw.default(...params),
			deferred: (...params: ArgumentTypes<F>) => raw.deferred(...params),
			immediate: (...params: ArgumentTypes<F>) => raw.immediate(...params),
			exclusive: (...params: ArgumentTypes<F>) => raw.exclusive(...params),
		});
		Object.defineProperty(audited, "database", { value: this, enumerable: true });
		return audited as AuditedTransaction<F>;
	}

	exec(source: string): this {
		this.#raw.exec(source);
		return this;
	}

	pragma(source: string, options?: import("better-sqlite3").PragmaOptions): unknown {
		return options === undefined ? this.#raw.pragma(source) : this.#raw.pragma(source, options);
	}

	function(name: string, cb: SqliteFunctionCallback): this;
	function(name: string, options: RegistrationOptions, cb: SqliteFunctionCallback): this;
	function(
		name: string,
		optionsOrCallback: RegistrationOptions | SqliteFunctionCallback,
		callback?: SqliteFunctionCallback,
	): this {
		if (typeof optionsOrCallback === "function") {
			this.#raw.function(name, optionsOrCallback);
		} else {
			this.#raw.function(name, optionsOrCallback, callback as SqliteFunctionCallback);
		}
		return this;
	}

	aggregate<T>(
		name: string,
		options: RegistrationOptions & {
			start?: T | (() => T);
			// biome-ignore lint/suspicious/noConfusingVoidType: Mirrors better-sqlite3's aggregate contract.
			step: (total: T, next: T extends Array<infer Element> ? Element : T) => T | void;
			inverse?: (total: T, dropped: T) => T;
			result?: (total: T) => unknown;
		},
	): this {
		this.#raw.aggregate(name, options);
		return this;
	}

	loadExtension(path: string): this {
		this.#raw.loadExtension(path);
		return this;
	}

	close(): this {
		this.#raw.close();
		return this;
	}

	defaultSafeIntegers(toggleState?: boolean): this {
		if (toggleState === undefined) this.#raw.defaultSafeIntegers();
		else this.#raw.defaultSafeIntegers(toggleState);
		return this;
	}

	backup(destinationFile: string, options?: BackupOptions): Promise<BackupMetadata> {
		return options === undefined
			? this.#raw.backup(destinationFile)
			: this.#raw.backup(destinationFile, options);
	}

	table(name: string, options: Parameters<RawDatabase["table"]>[1]): this {
		this.#raw.table(name, options);
		return this;
	}

	unsafeMode(unsafe?: boolean): this {
		if (unsafe === undefined) this.#raw.unsafeMode();
		else this.#raw.unsafeMode(unsafe);
		return this;
	}

	serialize(options?: SerializeOptions): Buffer {
		return options === undefined ? this.#raw.serialize() : this.#raw.serialize(options);
	}
}

class AuditedStatement<BindParameters extends unknown[], Result> {
	readonly #raw: RawStatement<BindParameters, Result>;
	readonly database: AuditedSqliteConnection;

	constructor(database: AuditedSqliteConnection, raw: RawStatement<BindParameters, Result>) {
		this.database = database;
		this.#raw = raw;
	}

	get source(): string {
		return this.#raw.source;
	}

	get reader(): boolean {
		return this.#raw.reader;
	}

	get readonly(): boolean {
		return this.#raw.readonly;
	}

	get busy(): boolean {
		return this.#raw.busy;
	}

	run(...params: BindParameters): import("better-sqlite3").RunResult {
		return this.#raw.run(...params);
	}

	get(...params: BindParameters): Result | undefined {
		return this.#raw.get(...params);
	}

	all(...params: BindParameters): Result[] {
		return this.#raw.all(...params);
	}

	iterate(...params: BindParameters): IterableIterator<Result> {
		return this.#raw.iterate(...params);
	}

	pluck(toggleState?: boolean): this {
		if (toggleState === undefined) this.#raw.pluck();
		else this.#raw.pluck(toggleState);
		return this;
	}

	expand(toggleState?: boolean): this {
		if (toggleState === undefined) this.#raw.expand();
		else this.#raw.expand(toggleState);
		return this;
	}

	raw(toggleState?: boolean): this {
		if (toggleState === undefined) this.#raw.raw();
		else this.#raw.raw(toggleState);
		return this;
	}

	bind(...params: BindParameters): this {
		this.#raw.bind(...params);
		return this;
	}

	columns(): import("better-sqlite3").ColumnDefinition[] {
		return this.#raw.columns();
	}

	safeIntegers(toggleState?: boolean): this {
		if (toggleState === undefined) this.#raw.safeIntegers();
		else this.#raw.safeIntegers(toggleState);
		return this;
	}
}

/** Sole owner of a write-capable SQLite connection within one process. */
export class WriterActor extends AuditedSqliteConnection {
	private constructor(raw: RawDatabase) {
		super(raw);
	}

	static open(dbPath: string): WriterActor {
		mkdirSync(dirname(dbPath), { recursive: true });
		const raw = new BetterSqlite3(dbPath);
		try {
			recordOpen(dbPath, "writer", "writer_actor");
			return new WriterActor(raw);
		} catch (error) {
			raw.close();
			throw error;
		}
	}
}

/** Audited transitional read handle; T048 removes daemon-external readers. */
export class ReadOnlyActor extends AuditedSqliteConnection {
	private constructor(raw: RawDatabase) {
		super(raw);
	}

	static open(dbPath: string): ReadOnlyActor {
		const raw = new BetterSqlite3(dbPath, { readonly: true, fileMustExist: true });
		try {
			recordOpen(dbPath, "readonly", "readonly_actor");
			return new ReadOnlyActor(raw);
		} catch (error) {
			raw.close();
			throw error;
		}
	}
}
