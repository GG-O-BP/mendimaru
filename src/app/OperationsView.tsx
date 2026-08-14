import type { LocalizationBundle } from "../domain/types";
import {
  OperationsPage,
  type OperationsPageModel,
} from "../features/operations/OperationsPage";
import type { useOperations } from "../features/operations/useOperations";
import type { Translate } from "../i18n";

export function OperationsView({
  t,
  localization,
  operations,
}: {
  t: Translate;
  localization: LocalizationBundle;
  operations: ReturnType<typeof useOperations>;
}) {
  const model: OperationsPageModel = {
    operations: operations.operations,
    loading: operations.loading,
    isBusy: operations.isBusy,
    onRefresh: () => void operations.refresh(),
    onRetry: operations.retry,
    onClear: operations.clear,
    onOpenLogs: () => void operations.openLogs(),
  };
  return <OperationsPage t={t} localization={localization} model={model} />;
}
