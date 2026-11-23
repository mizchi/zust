// Types outside what zsc reads from TS (tests/errors/ts_*.zsc): each import of one is an error at its place here
export interface Opt { name?: string }
export interface Uni { status: "a" | "b" }
export interface Box<T> { value: T }
export interface Base { id: number }
export interface Sub extends Base { name: string }
export interface When { at: Date }
export interface Method { run(): void }
interface Hidden { x: number }
export interface UsesHidden { h: Hidden }
export type Either = { a: number } | { b: string };
export interface Twice { a: number }
export interface Twice { b: string }
export type Cond = { a: number } extends object ? { b: string } : never;
