# T4 Publish Notes

## Crates to publish

Publish the T4 chain in this order:

1. `tg-rcore-tutorial-sbi`
2. `tg-rcore-tutorial-task-manage`
3. `exp4-scheduler`
4. `exp5-sync`
5. `syscall-t3l8`
6. `tg-rcore-tutorial-user`
7. `tg-rcore-tutorial-ch8`

The current package names and versions are:

- `jiaxin2006-tg-rcore-tutorial-sbi-t4 = 0.0.4-preview.1`
- `jiaxin2006-tg-rcore-tutorial-task-manage-t4 = 0.0.4-preview.1`
- `jiaxin2006-tg-rcore-tutorial-t2l4 = 0.0.4-preview.1`
- `jiaxin2006-tg-rcore-tutorial-t2l5 = 0.0.4-preview.1`
- `jiaxin2006-tg-rcore-tutorial-syscall-t4 = 0.0.4-preview.1`
- `jiaxin2006-tg-rcore-tutorial-user-t4 = 0.0.4-preview.1`
- `jiaxin2006-tg-rcore-tutorial-t4 = 0.0.4-preview.1`

`tg-rcore-tutorial-easy-fs` is still referenced as `jiaxin2006-tg-rcore-tutorial-easy-fs-t3l8 = 0.0.1-preview.1`. If that crate has not been published from this repository yet, publish it before `tg-rcore-tutorial-ch8`.

## Recommended commands

Dry-run the whole chain first:

```bash
bash scripts/publish-t4.sh dry-run
```

This dry-run uses `cargo package --allow-dirty --list`. It is meant to confirm that each crate can be packaged locally and that the expected files are included before the first publish. For a brand-new dependency chain, `cargo publish --dry-run` on later crates will still fail until earlier crates have actually appeared in the crates.io index.

If you want to publish for real:

```bash
cargo login <your-crates-io-token>
bash scripts/publish-t4.sh publish
```

If you prefer manual commands:

```bash
cd tg-rcore-tutorial-sbi && cargo publish --allow-dirty
cd ../tg-rcore-tutorial-task-manage && cargo publish --allow-dirty
cd ../exp4-scheduler && cargo publish --allow-dirty
cd ../exp5-sync && cargo publish --allow-dirty
cd ../syscall-t3l8 && cargo publish --allow-dirty
cd ../tg-rcore-tutorial-user && cargo publish --allow-dirty
cd ../tg-rcore-tutorial-ch8 && cargo publish --allow-dirty
```

Wait a few seconds between publishes so downstream dependencies become visible in the index.
