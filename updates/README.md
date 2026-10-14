# Ortak güncelleme altyapısı

Bu klasör aynı repository içinde bağımsız Rust workspace'tir. Uygulama/backend workspace'ine ve Tauri kaynaklarına bağlı değildir. Bütün Cargo komutları `updates/` içinde çalıştırılır. Rust 1.97.1 kullanılır.

| Paket | Görev |
|---|---|
| `crates/update-protocol` | Şema v1, kanonik imzalar, release/descriptor, checkpoint, rollout ve lifecycle kuralları |
| `crates/update-client` | Güvenilen metadata, full/delta indirme, doğrulama, kurulum kilidi ve restart sonrası uzlaştırma |
| `services/update-server` | Axum API, PostgreSQL katalog/Admin transaction'ları, özel R2'ye 307 yönlendirme |
| `tools/release-cli` | Build, offline imza, full/delta üretimi, değişmez upload, draft/publish/promote ve yaşam döngüsü |

## Yerel doğrulama

```powershell
cd updates
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

PostgreSQL testi isteğe bağlı ayrı ve geçici veritabanı ister. Docker'ı yalnız kullanıcı/operatör açar:

```powershell
docker compose -f deploy/postgres-test.yml up -d --wait
$env:UPDATE_TEST_DATABASE_URL = 'postgres://updates_test:local-test-only@127.0.0.1:18432/updates_test'
cargo test -p update-server --test postgres --locked -- --ignored
docker compose -f deploy/postgres-test.yml stop
```

`tests/deploy_test.py` Linux'ta deploy komutunu sahte Docker/HAProxy/curl uçlarıyla sınar; gerçek Docker açmaz. Gerçek VPS/R2 kabulünün yerine geçmez. PostgreSQL ve container/VPS testleri çalıştırılmadığında ayrıca raporlanır. [Doğrulama kaydı](../docs/planlama/dogrulama/update-altyapisi.md).

## Anahtarlar ve server yapılandırması

`deploy/config.example.json` iki uygulamalı **şablondur**; `REPLACE_*` alanlarıyla çalışmaz. `deploy/config.json` kopyasını oluşturup uygulamaları, kanalları, hedefleri, ayrı bucket'ları ve public anahtarları doldurun. Yeni uygulama kod değiştirmeden bu kayıtla eklenir; config değişikliği iki slotta aynı revizyonla dağıtılır. Container'lar config'i başlangıçta okur.

```powershell
cargo run -p release-cli -- keygen --out secrets/baykus-metadata.key --key-id baykus-2026
$env:UPDATE_TOKEN = '<rastgele uzun admin veya özel kanal tokeni>'
cargo run -p release-cli -- token-hash
cargo run -p update-server -- --config deploy/config.json validate
```

`keygen` çıktısı yalnız public key haritasıdır; bunu config ve client'a sabitleyin. Metadata private key'i developer/release ortamında kalır. Tauri updater anahtarları ve Windows yayıncı sertifikası ayrı anahtarlardır; paket `.sig` dosyası Tauri build/signer çıktısıdır. Her uygulama kendi trust setini kullanır. Public-key rotasyonunda önce eski client'ların yeni anahtara güvenebildiği uygulama sürümü dağıtılır; server yanıtından public key kabul edilmez.

`UPDATE_ADMIN_TOKEN` ve özel kanal tokenları ayrı yetkilerdir. Server yalnız SHA-256 token hash'lerini tutar. R2 runtime credentials yalnız uygulamaların update bucket'larını **okuyabilmelidir**; upload credentials release ortamında kalır. Upload adımı her uygulamanın `app_id/health.txt` nesnesini de hazırlar.

Native server:

```powershell
$env:DATABASE_URL = '<update_migrator için PostgreSQL URL>'
cargo run -p update-server -- migrate
$env:DATABASE_URL = '<update_runtime için PostgreSQL URL>'
$env:R2_ACCESS_KEY_ID = '<read-only access key>'
$env:R2_SECRET_ACCESS_KEY = '<read-only secret>'
cargo run -p update-server -- --config deploy/config.json serve --bind 127.0.0.1:8088
```

`deploy/grants.sql` runtime rolüne katalog işlemleri ve append-only audit yetkisini verir; şema/deletion yetkisi vermez. Native kurulumda rolleri ayrıca oluşturun ve grant dosyasını migration rolüyle uygulayın. Sunucu startup'ta migration yapmaz. DB, geçerli config ve bütün uygulamaların R2 health nesneleri erişilebilir değilse readiness `503` olur.

## Release aracı

```powershell
cargo run -p release-cli -- prepare `
  --app-id baykuslig --channel staging --target windows-x86_64-nsis `
  --version 0.2.0 --sequence 2 --package build/app-setup.exe `
  --package-signature build/app-setup.exe.sig --package-public-key secrets/tauri.pub `
  --metadata-key secrets/baykus-metadata.key --key-id baykus-2026 `
  --base releases/0.1.0/full.bin --out releases/0.2.0

cargo run -p release-cli -- verify --bundle releases/0.2.0/bundle.json --public-key metadata-public-keys.json

# R2_ENDPOINT / R2_ACCESS_KEY_ID / R2_SECRET_ACCESS_KEY: ayrı upload yetkisi
cargo run -p release-cli -- upload --bundle releases/0.2.0/bundle.json `
  --files releases/0.2.0 --bucket baykuslig-updates --public-key metadata-public-keys.json

# UPDATE_ADMIN_TOKEN: ilgili uygulama/kanal yetkisi
cargo run -p release-cli -- draft --server https://updates.example.com `
  --bundle releases/0.2.0/bundle.json

cargo run -p release-cli -- action --server https://updates.example.com `
  --app-id baykuslig --channel staging --target windows-x86_64-nsis `
  --release-id '<bundle içindeki release_id>' --action publish --operation-id publish-0.2.0
```

`--base` isteğe bağlıdır; yalnız patch tam paketten küçükse delta üretilir. Format, Zstandard 0.13 referans-dictionary frame'idir; üretici ve client aynı motoru kullanır. Resmi zstd CLI `patch-from` çıktısının bütün seçenekleriyle byte uyumu iddia edilmez. İmzalı descriptor tam taban/sonuç hash ve boyutunu bağlar. Tam paketin `.sig` imzası delta sonucuna da uygulanır.

`--build-program` ve tekrarlanan `--build-arg` Tauri CLI dahil ayrı build programını shell interpolation olmadan çalıştırır; prepare mevcut imzalı build'i kullanabilir. Metadata imzası offline üretilir; upload/admin adımları daha sonra başka yetkili ortamda çalışabilir. Prepare çıktı dizinini ve mevcut private key'i overwrite etmez. Upload `If-None-Match: *` kullanır; mevcut nesne ancak hash/boyut eşleşirse kabul edilir.

Yerel test sunucusunda `draft`, `action` ve `promote` için `--ca-certificate root.crt` eklenebilir. Bu seçenek yalnız verilen CA'yı ek güven kökü olarak tanıtır; HTTPS sunucu adı ve sertifika doğrulaması açık kalır.

İlk draft politikası `%100`dür. Kademeli yayında publish **öncesinde** `action --action rollout --percentage 10 --cohort-key <sabit kohort> --revision 1` kullanın. Sonraki revizyonlar tam `+1` olur; kohort anahtarı ilk politika değişiminden sonra sabittir. `--paused` dağıtımı duraklatır. Aynı işlem yeniden denenirken aynı operation ID ve aynı parametreler kullanılır.

`publish`, `disable`, `enable`, `revoke` işlemleri bu action komutuyla yürütülür; revoke terminaldir. `promote --source-bundle ... --channel production --sequence ... --metadata-key ... --key-id ... --public-key metadata-public-keys.json --out ...` artifact'leri değiştirmeden yeni imzalı manifest ve draft oluşturur. Kaynak checkpoint içeriyorsa hedef kanaldaki `--checkpoint version:release_id` eşlemeleri gerekir. Checkpoint'siz promotion buna ihtiyaç duymaz. Publish ayrıca yapılır. Promotion isteği ağda başarısız olursa kaydedilmiş bundle ile `draft` yeniden denenebilir; yeni manifest üretmek gerekmez.

## Windows installer release'i

`ReleaseManifest.installation` isteğe bağlı imzalı dosya envanteri, yayıncı sertifika SHA-256 pini, Windows/WebView2 gereksinimleri ve staging/aktif/yedek alan bütçelerini taşır. Eski envantersiz release'ler okunabilir; yeni Windows servis akışı envanteri zorunlu ister.

Son imzalı uygulama dosyalarından paket üretin:

```powershell
cargo run -p release-cli -- windows-package `
  --payload build/final-signed-payload --app-id baykuslig `
  --product-directory BaykusLig --executable baykuslig.exe --version 0.2.0 `
  --maintenance-executable maintenance.exe `
  --publisher-certificate-sha256 '<sertifikanın SHA-256 değeri>' `
  --minimum-windows-build 10240 --minimum-webview2-version 0.0.0.0 `
  --out build/windows-package
makensis build/windows-package/package.nsi
```

Bu örnekte `maintenance.exe`, payload köküne eklenen son imzalı installer arayüzüdür; `--repair` ve `--uninstall` girişlerini desteklemelidir. NSIS bunu korunan servis klasörüne de kopyalar ve Windows Değiştir/Kaldır kayıtlarını bu arayüze bağlar. Dosya imzalı envanter ve disk bütçesine dahil edilir. Bakım EXE'si parametresi verilmezse eski sessiz helper uninstall girişi korunur; Değiştir etkinleştirilmez.

Ürün runtime alt sınırlarını gerçek dağıtım gereksinimlerine göre verin. Üretilen installer'ı Authenticode ile, sonra Tauri updater signer ile imzalayın. Yukarıdaki `prepare` komutuna `--installation-contract build/windows-package/contract.json` ekleyin; `--package` ve `.sig` yollarını son imzalı installer'a yöneltin. Payload envanter üretiminden sonra değişirse yeniden üretin. `windows-package` imza atmaz; gerçek private key/sertifikaları operatör sağlar.

Helper `UPDATE_SERVICE_BOOTSTRAP_KEYS` JSON public-key haritasıyla derlenir ve Authenticode imzalanır. `release-cli sign-windows-policy --policy policy.json --metadata-key secrets/baykus-metadata.key --key-id baykus-2026 --out signed-policy.json` offline politikayı imzalar. İlk yönetici bootstrap'ı `update-service register signed-policy.json` çalıştırır; sonraki uygulama güncellemeleri yerel servisle yürür. Farklı helper/politika üzerine otomatik yazılmaz; servis güncellemesi ayrı bakım dağıtımı gerektirir.

Server `POST /v1/install/resolve` ile ilk kurulum/update/repair seçer; `/v1/install/authorize` son lifecycle/rollout kontrolünden sonra helper nonce'una bağlı ayrı imzalı izin üretir. Online kontrol normal uygulamadadır. Ayrı operasyonel anahtar dağıtımı [deploy yönergesindedir](./deploy/README.md#kurulum-yetkisi-anahtarı). [Uygulama sırası ve kilit zorunluluğu](./crates/update-client/README.md), [kanıt/açık kabul](../docs/planlama/dogrulama/update-client-installer.md).

## Tauri ajanına entegrasyon sözleşmesi

Windows adaptörü ve offline perMachine servis temeli bu crate içinde uygulandı; Linux/macOS adaptörleri hâlâ açıktır. [Client yönergesindeki](./crates/update-client/README.md) servis akışı kullanılır. App ajanı bildirim/onay, ömür boyu `application_guard`, güvenli çıkış ve normal kullanıcı restart'ını bağlar. Bu bağlantılar ve gerçek imzalı Windows kabulü tamamlanmış sayılmaz. Uygulama indirir ve son online kontrolü yapar; servis korumalı tam paket/metadata/kurulum iznini bağımsız doğrular. [DEC-C055](../docs/baykuslig/kararlar/kapatilan-kararlar-c055.md), [DEC-C056](../docs/baykuslig/kararlar/kapatilan-kararlar-c056.md), [platform politikası](../docs/baykuslig/urun/guncellemeler-ve-platform-destegi.md) ve [offline servis sözleşmesi](../docs/ortak-servisler/windows-offline-kurulum-servisi.md) geçerlidir.

Baykuş Lig aynı repo içinden `path = "../../../updates/crates/update-client"` kullanabilir; gerçek manifest konumuna göre yolu ayarlayın. Diğer projeler:

```toml
update-client = { git = "ssh://git@github.com/OWNER/PRIVATE_REPO.git", branch = "stable" }
```

`cargo update -p update-client` test edilmiş yeni commit'i lock dosyasına alır. Güncel [CI/CD sürecinde](../docs/planlama/ci-cd.md) tag push otomatik yayın başlatmaz: tam CI sabit main SHA'sı için manuel çalışır; başarılı koşunun mevcut GHCR imajı ayrı manuel publisher ile yeniden build olmadan yayımlanır. Stable branch gerekiyorsa test edilmiş commit'e ayrıca manuel fast-forward yapılır; başka biçimde değiştirilmez. Cargo güncellemesi VPS deploy etmez.

## Docker ve blue/green

[VPS yönergesi](./deploy/README.md) bootstrap/deploy/rollback, sırlar, migration, atomik HAProxy map geçişi ve drain işlemlerini içerir. Compose kendi PostgreSQL, sabit Caddy TLS proxy, HAProxy router ve iki updater slotunu taşır; app/backend container'larını çalıştırmaz veya yeniden oluşturmaz. Caddy TLS frontend deploy sırasında sabit kalır; HAProxy runtime map yalnız updater hedefini atomik değiştirir. Mevcut ortak TLS proxy bu router’a yönlendirilebilir.

## HTTP ve imza

Kanonik sözleşme [ortak update belgesidir](../docs/ortak-servisler/update-server.md). `POST /v1/update/check` ile tam/delta seçilir; ortak GET ve `/v1/tauri/...` full paket sunar. No update `204`; bozuk katalog/DB/R2 `503`dür. İndirme endpoint'i lifecycle/yetki kontrolünden sonra `307` döndürür; paket doğrudan private R2'dendir. Client özel kanal tokenını R2'ye taşımaz ve redirect takibini kendi yapar.

İmzalı zarflar `{key_id, body, signature}` şeklindedir. Body RFC 8785/JCS ile kanonikleşir. Ed25519 imza byte'ları `update-system/release/v1\n` veya `update-system/artifact/v1\n` + kanonik body'dir. Manifest her descriptor body'sinin SHA-256'sını içerir. Metadata public key 32 byte, signature 64 byte, standart base64; hash lowercase SHA-256'dır. Sürüm SemVer, build metadata yok; sequence `1..=9007199254740991` olur. Lifecycle ve rollout imzalı metadata'dan ayrıdır. `published_at` hazırlama sırasında sabitlenen metadata zamanıdır; operasyonel publish/audit zamanı DB'de ayrıca tutulur.
