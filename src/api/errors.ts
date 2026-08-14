import type { CommandError, CommandErrorCode } from "../domain/types";
import type { Translate } from "../i18n";
import enumValues from "../shared/contracts/enumValues.json";

const COMMAND_ERROR_CODES = new Set<CommandErrorCode>(
  Object.keys(enumValues.commandErrorCode) as CommandErrorCode[],
);

export function errorText(error: unknown, t: Translate): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  if (isCommandError(error)) return error.message;
  try {
    return JSON.stringify(error) ?? t("unknown-error");
  } catch {
    return t("unknown-error");
  }
}

export function errorCode(error: unknown): CommandErrorCode | undefined {
  return isCommandError(error) ? error.code : undefined;
}

export function isCommandError(error: unknown): error is CommandError {
  return Boolean(
    error &&
    typeof error === "object" &&
    "code" in error &&
    typeof error.code === "string" &&
    COMMAND_ERROR_CODES.has(error.code as CommandErrorCode) &&
    "message" in error &&
    typeof error.message === "string",
  );
}
