export function errorMessage(error: unknown, fallback: string): string {
  if (typeof error === "string" && error.length > 0) return error;
  if (error instanceof Error && error.message.length > 0) return error.message;
  return fallback;
}
