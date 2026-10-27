//! Almacén de secretos: desbloqueo, guardado, cambio de modo y migración.

use super::*;

impl App {
    /// Clave maestra en uso: la del almacén activo o la de una migración en curso.
    pub(super) fn active_master(&self) -> Option<Arc<MasterKeyStore>> {
        self.secrets.master().cloned().or_else(|| self.migration_master.clone())
    }

    /// Cuentas bajo las que puede haber contraseñas o passphrases guardadas.
    pub(super) fn secret_accounts(&self) -> Vec<String> {
        self.connections
            .iter()
            .filter_map(|c| c.id)
            .flat_map(|id| [account_for(id, SecretKind::Password), account_for(id, SecretKind::Passphrase)])
            .collect()
    }

    /// Cuántos secretos guardados hay en el almacén activo.
    pub(super) fn stored_secret_count(&self) -> usize {
        if self.secrets.master().is_some() {
            return self.store.lock().unwrap().secret_count().unwrap_or(0);
        }
        self.secret_accounts().iter().filter(|a| matches!(KeyringStore.get(a), Ok(Some(_)))).count()
    }

    pub(super) fn begin_migration(&mut self, from: MigrateFrom) {
        Secrets::set_pending_migration(&self.store.lock().unwrap(), None).ok();
        let present = match from {
            MigrateFrom::Master => self.store.lock().unwrap().secret_count().unwrap_or(0) > 0,
            MigrateFrom::Keyring => self.secret_accounts().iter().any(|a| matches!(KeyringStore.get(a), Ok(Some(_)))),
        };
        if !present {
            return;
        }
        if self.active_master().is_none() {
            self.migration_master = Some(Arc::new(MasterKeyStore::new(self.store.clone())));
        }
        if self.active_master().is_some_and(|m| m.is_unlocked()) {
            self.run_migration(from);
        } else {
            self.ask_unlock(Pending::Migrate(from));
        }
    }

    pub(super) fn run_migration(&mut self, from: MigrateFrom) {
        let Some(master) = self.active_master() else { return };
        let accounts = self.secret_accounts();
        let result = match from {
            MigrateFrom::Keyring => crate::data::secrets::migrate(&KeyringStore, master.as_ref(), &accounts),
            MigrateFrom::Master => crate::data::secrets::migrate(master.as_ref(), &KeyringStore, &accounts),
        };
        match result {
            Ok(1) => self.notify("Se migró 1 contraseña al nuevo almacén"),
            Ok(count) => self.notify(format!("Se migraron {count} contraseñas al nuevo almacén")),
            Err(error) => self.dialog = Some(Dialog::failure("No se pudieron migrar las contraseñas", error)),
        }
    }

    /// Aplica el cambio de almacén elegido; con `migrate`, las contraseñas se copian en el próximo arranque.
    pub(super) fn apply_secrets_mode(&mut self, to_master: bool, migrate: bool) {
        let active_master = self.secrets.master().is_some();
        let store = self.store.lock().unwrap();
        let result = Secrets::set_master_forced(&store, to_master).and_then(|()| {
            let from = (migrate && to_master != active_master).then_some(if active_master { MigrateFrom::Master } else { MigrateFrom::Keyring });
            Secrets::set_pending_migration(&store, from)
        });
        drop(store);
        match result {
            Err(error) => self.dialog = Some(Dialog::failure("No se pudo guardar el ajuste", format!("{error:#}"))),
            Ok(()) if to_master == active_master => self.notify("Se mantiene el almacén actual"),
            Ok(()) => self.dialog = Some(Dialog::Restart),
        }
    }

    pub(super) fn restart_app(&mut self) {
        let spawned = std::env::current_exe().and_then(|exe| std::process::Command::new(exe).env(RESTARTED_ENV, "1").spawn());
        match spawned {
            Ok(_) => {
                self.close_confirmed = true;
                self.ctx.send_viewport_cmd(ViewportCommand::Close);
            }
            Err(error) => self.dialog = Some(Dialog::failure("No se pudo reiniciar", format!("{error:#}"))),
        }
    }

    pub(super) fn ask_unlock(&mut self, then: Pending) {
        let creating = self.active_master().is_some_and(|m| !m.is_initialized().unwrap_or(false));
        self.dialog = Some(Dialog::Unlock(UnlockPrompt {
            creating,
            reveal: false,
            input: String::new(),
            confirm: String::new(),
            error: None,
            then,
            focus: true,
        }));
    }

    pub(super) fn save_secret(&mut self, account: String, secret: Zeroizing<String>) {
        match self.secrets.set(&account, &secret) {
            Ok(()) => {}
            Err(SecretError::Locked) => self.ask_unlock(Pending::Save { account, secret }),
            Err(error) => self.dialog = Some(Dialog::failure("No se pudo guardar el secreto", error)),
        }
    }
}
