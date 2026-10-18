/** Customized builds are updated manually, never from the upstream feed. */
const DISABLED_MESSAGE = '此中文定制版已禁用自动更新，请从本分支下载新的安装包并手动安装。';

class Updater {
    constructor() {
        this.updateAvailable = null;
        this.onUpdateFound = null;
        this.onCheckComplete = null;
        this.onError = null;
    }

    async checkForUpdates() {
        this.updateAvailable = null;
        if (this.onCheckComplete) this.onCheckComplete(false);
        // Existing callers use false to display "up to date". Replace that
        // status after the callback: no remote version check was performed.
        const status = globalThis.document?.getElementById('update-status-text');
        if (status) status.textContent = DISABLED_MESSAGE;
        console.info(`[Updater] ${DISABLED_MESSAGE}`);
        return { disabled: true, message: DISABLED_MESSAGE };
    }

    async downloadAndInstall() {
        throw new Error(DISABLED_MESSAGE);
    }
}

export const updater = new Updater();
