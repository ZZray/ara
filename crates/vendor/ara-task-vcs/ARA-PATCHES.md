# Fixed consumed Task Git source

OMP b91db66e complete eight Git modules plus error/types. Task-only lib facade replaces full Git/Jj dispatch. jj/mod.rs filesystem discovery body preserved without operations. Added GitRepo::compare_and_swap_ref via existing hardened atomic git update-ref runner for exact resource-owner publication; no forced public ref replacement. New facade/CAS tests authored; original Git tests remain. Not full pi-vcs/Jujutsu acceptance.

Two uses of the unstable `Option::map_or_default` in `git/patch.rs` use stable equivalents: an absent first patch line yields `""`, and an absent alternate index yields an empty `BTreeMap`. Present values keep the original closures. The first module build failed with E0658 at both original calls; the failure packet is retained. This compatibility change does not establish runtime acceptance.
