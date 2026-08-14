import { useCallback, useState } from "react";
import { errorText } from "./api/errors";
import { Workspace } from "./app/Workspace";
import { useLocalization } from "./features/localization/useLocalization";
import { useEnvironment } from "./features/settings/useEnvironment";
import {
  ConfigurationRecovery,
  ConfirmDialog,
  LoadingScreen,
  ToastStack,
} from "./shared/components/Feedback";
import { useActionRegistry } from "./shared/hooks/useActionRegistry";
import { useFeedback } from "./shared/hooks/useFeedback";
import "./App.css";

function App() {
  const [warning, setWarning] = useState<string | null>(null);
  const {
    toasts,
    confirmation,
    notify,
    dismissToast,
    requestConfirmation,
    cancelConfirmation,
    acceptConfirmation,
    clearFeedback,
  } = useFeedback();
  const handleLocalizationError = useCallback((error: unknown) => {
    setWarning(error instanceof Error ? error.message : String(error));
  }, []);
  const localization = useLocalization(handleLocalizationError);
  const { t } = localization;
  const handleActionError = useCallback(
    (error: unknown) => {
      notify("error", t("generic-action-failed"), errorText(error, t));
    },
    [notify, t],
  );
  const { runAction, isBusy, hasBusyPrefix } =
    useActionRegistry(handleActionError);
  const onWarning = useCallback(
    (message: string | null) => setWarning(message),
    [],
  );
  const environment = useEnvironment({
    t,
    notify,
    requestConfirmation,
    runAction,
    isBusy,
    onWarning,
  });

  if (!localization.localization || environment.loading) {
    return <LoadingScreen />;
  }

  if (!environment.config) {
    return (
      <>
        <ConfigurationRecovery
          t={t}
          detail={warning}
          busy={isBusy("redetect")}
          onRecover={() => void environment.redetectSettings()}
        />
        <ToastStack t={t} toasts={toasts} onDismiss={dismissToast} />
      </>
    );
  }

  return (
    <>
      <Workspace
        t={t}
        localization={localization.localization}
        languageChanging={localization.changing}
        warning={warning}
        environment={environment}
        notify={notify}
        requestConfirmation={requestConfirmation}
        runAction={runAction}
        isBusy={isBusy}
        hasBusyPrefix={hasBusyPrefix}
        selectLanguage={localization.changeLanguage}
        clearFeedback={clearFeedback}
        onWarning={onWarning}
      />
      <ToastStack t={t} toasts={toasts} onDismiss={dismissToast} />
      {confirmation && (
        <ConfirmDialog
          t={t}
          state={confirmation}
          onCancel={cancelConfirmation}
          onConfirm={acceptConfirmation}
        />
      )}
    </>
  );
}

export default App;
