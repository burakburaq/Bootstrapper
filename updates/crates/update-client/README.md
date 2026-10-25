# App ve installer entegrasyonu

Bu crate uygulama/Tauri bağımlılığı içermez. `ClientConfig` sabit `app_id/channel/target`, HTTPS origin, metadata public anahtarları, Tauri paket public key'i, izinli indirme hostları ve normal kullanıcı cache'ini taşır. Production'da `allow_loopback_http = false` olur. UI anahtar, paket/hedef yolu, sürüm, kurulum kimliği veya komut satırı vermez; native kod bunları güvenilen yapılandırma ve servisten alır.

## Windows perMachine akışı

`update-service` bu crate'in Windows binary'sidir; workspace dört pakette kalır. LocalSystem servisi ağ kullanmadan paket, metadata, envanter ve kısa ömürlü kurulum iznini bağımsız doğrular. Uygulama ve WebView normal kullanıcı yetkisinde çalışır. İlk servis bootstrap'ı UAC ister; kayıtlı servis üzerinden sonraki update/repair işlemleri yeni UAC istemez. Gerçek imzalı dağıtım ve Windows VM kabulü henüz tamamlanmamıştır.

Native entegrasyon sırası:

1. Uygulama, kullanıcı veritabanını veya pencereleri açmadan önce `windows::application_guard(&policy, &compiled_version)` çağırır ve dönen `File` değerini uygulama ömrü boyunca tutar. Servis aktivasyonda aynı kapıyı özel kilitler. Guard gerçek EXE sürümü/yayıncı yanında korumalı head journal'daki son doğrulanmış release'in imzalı dosya envanterini denetler; yarım aktivasyondaki eksik DLL'lerle açılışa izin vermez. Açılış beklerken sürüm değişmişse süreç normal kullanıcı olarak yeniden başlatılmalıdır. Bu bağlantı atlanamaz.
2. `WindowsServiceClient { policy }` oluşturulur. `pending()` ve `query(operation_id)` salt okunur durum verir. `installation_context()` gerçek `Option<Version>` ve korumalı perMachine `installation_id` döndürür; registry alanları tek başına kanıt değildir.
3. `check_installation(installed, installation_id, intent, operation_id, callback)` çağrılır. `InstallIntent::Install` yalnız kurulum yokken, `Update` yalnız daha yeni sürüm için, `Repair` aynı sürüm için kullanılır. `204` yerel dosyaların sağlam olduğunu kanıtlamaz.
4. Sonuç `prepare_with_progress(update, base, operation_id, intent, callback)` ile normal cache'e indirilip doğrulanır. Windows servis akışı tam paket kullanır. Hazırlanmış paket kurulum başarısı değildir.
5. Kullanıcı onayından sonra `service.begin(&prepared, operation_id, intent)` çağrılır. `BeginResult::Existing(event)` mevcut işlem/no-op sonucudur. `BeginResult::Challenge(challenge)` için normal uygulama `client.authorize_installation(&challenge.request)` çağırır; ardından `service.commit(&prepared, operation_id, authorization, callback)` paket byte'larını yerel pipe üzerinden teslim eder.
6. `ClosingApplications` geldiğinde uygulama kullanıcı işini kaydedip kapanır; yaşam boyu guard serbest kalır. Servis kapının açılmasını en fazla 30 saniye bekler, başka oturumları zorla kapatmaz. Kabul edilmiş kurulum istemcinin pipe bağlantısı kapanınca da serviste devam eder. Installer tüm oturumlarda güvenli kapanmayı ve normal kullanıcı restart'ını bağlar. Başarı yalnız `Completed`/`NoOp` sonucundan sonra gösterilir; `Failed` veya `Interrupted` başarı değildir.

Challenge içindeki `preflight: PreflightReport`, UI'nin gösterebileceği gerçek `required_total_bytes`, `available_staging_bytes`, `available_active_bytes`, `native_target`, `windows_build`, `webview2_version` ve `paths_writable` alanlarını taşır. WebView2 alt sınırı istenmediyse sürüm `None` olabilir. Servis commit öncesinde önkontrolü tekrarlar. Staging ve Program Files hedefi aynı volume'da olmalıdır; NSIS yedekleme rename adımı volume'lar arasında çalışamaz.

`begin` nonce üretir; server'ın ayrı operasyonel anahtarla imzaladığı izin kurulum/işlem/niyet/release/hash'e bağlanır. Servis tazeliği kendi monoton süresiyle ölçer; izni paket byte'larını almadan önce ve commit öncesinde yeniden denetler. UI veya servis online izin üretmez. Pipe yalnız yerel istemciye açıktır; süreç kimliği ve Authenticode sertifika SHA-256 pini kontrol edilir. Kullanıcı cache yolu yetkili çalıştırma kaynağı olarak kabul edilmez.

Servis known-folder API ile `ProgramData\Temp\Installer\<app_id>\<operation_id>\` staging ve korumalı ProgramData durum alanını kurar. ACL, reparse/UNC ve handle kontrolleri uygulanır. NSIS önce yalnız staging'e açar; imzalı manifestteki dosya yolu/boyut/SHA-256 envanteri doğrulanır. Aktivasyon sonrasında dosyalar, gerçek EXE sürümü/mimarisi, Authenticode, HKLM program kaydı ve SCM servis kaydı yeniden doğrulanır. NSIS exit code tek başına başarı değildir. Kullanıcı verisi payload envanterine girmez.

## İlerleme, journal ve yeniden deneme

`ProgressEvent` işlem kimliği, niyet, aşama, isteğe bağlı gerçek byte/toplam/yüzde ve hata taşır. `Checking`, `Downloading`, `VerifyingPackage`, `Preflight`, `AwaitingAuthorization`, `Staging`, `VerifyingStaging`, `ClosingApplications`, `Installing`, `VerifyingInstallation` ve son durumlar ayrı izlenir. Ölçülemeyen aşamalarda `percent = None` kalır; zamanla yüzde uydurulmaz.

`InstallJournal.operation` mevcut journal sözleşmesini genişletir. Servis korumalı `install.json` ile `operations/<operation_id>.json` kayıtlarını aynı tipte tutar; bunlar kesinti kurtarma durumudur, ayrı tanılama log'u değildir. Durum kayıtları normal kullanıcıya salt okunur, yalnız SYSTEM/yöneticilere yazılabilirdir; kullanıcı verisi içermez. Aynı kimlik/niyet tekrarında mevcut sonuç döner; farklı niyet/bundle reddedilir. Startup reconcile gerçek kurulu ağacı ve kayıtları inceler; kesilmiş işlem otomatik başarı veya downgrade olmaz. NSIS Job Object helper kapanırsa alt süreci sonlandırır. Servis yeniden başlarsa bekleyen izin yeniden alınır. Journal silerek rollback yapılmaz.

Update/repair `installation_id` değerini korur. Başarılı tam kaldırma mevcut journal'da kimliği emekliye ayırır ve son sağlam release kaydını temizler. Sonraki bootstrap/yeni kurulum bağlamında UUID atomik yenilenir; çalışan servis bağlamını yenileyip eski kimliğin challenge'larını atar. Helper ve anti-rollback tabanı kalır; kullanıcı verisi silinmez. Kesilmiş kaldırma başarı/kimlik yenileme sayılmaz. Bu yaşam döngüsünün gerçek Windows kabulü açıktır.

## İmzalı servis bootstrap'ı ve paket üretimi

Release ortamı helper'ı `UPDATE_SERVICE_BOOTSTRAP_KEYS` derleme değişkeninde JSON public-key haritasıyla üretir ve Authenticode imzalar. `WindowsPolicy` ürün kimliği/yolları, metadata ve ayrı authorization public-key haritaları, paket public key'i ve sertifika pinini taşır. `release-cli sign-windows-policy --policy ... --metadata-key ... --key-id ... --out ...` offline imza üretir. İlk yetkili bootstrap `update-service register <signed-policy.json>` çalıştırır. Sidecar kendi güven kökünü belirleyemez. Mevcut helper veya politika farklıysa kayıt işlemi üzerine yazmaz; ayrı servis bakım dağıtımı gerekir.

Son imzalı payload üzerinden `release-cli windows-package` envanterli `contract.json` ve stage/activate destekli `package.nsi` üretir. Sıra: payload imzala → script üret → makensis → installer Authenticode imzası → Tauri updater `.sig` → `release-cli prepare --installation-contract contract.json`. Envanter üretildikten sonra payload değiştirilmez. [Yayın/operasyon yönergesi](../../README.md), [kanıt ve açık kabul](../../../docs/planlama/dogrulama/update-client-installer.md).

## Mevcut genel API

`check` ve `prepare` korunur. Envantersiz eski entegrasyon için genel `Installer` trait'i ile `install`/`InstallJournal::reconcile` mevcuttur. Envanterli Windows release'leri eski kurulum yoluyla çalıştırılamaz; `WindowsServiceClient` kullanılır. Linux/macOS için gerçek platform adaptörleri ve platform kabulü hâlâ açıktır. Standart Tauri updater HTTP endpoint'i ortak client'ın imzalı metadata/journal/servis motorunu tek başına çalıştırmaz.

## Yerel HTTPS testi

Bağımlılıkta yalnız test için `features = ["local-test-tls"]` ve `UpdateClient::new_with_local_ca(config, &ca_pem)` kullanılır. Açık CA yalnız HTTPS localhost, loopback veya `.test` control origin'ine eklenir; hostname/zincir/süre kontrolü sürer. R2 ayrı standart trust kullanır. Production standart `UpdateClient::new` kullanır; CA remote yanıttan alınmaz, private CA key paylaşılmaz.

Proje kökünden boş katalog TLS smoke:

```powershell
cargo run --manifest-path updates/Cargo.toml -p update-client --features local-test-tls --example check-local -- updates/deploy/local/config.json updates/deploy/local/root.crt https://localhost:8443/
```

Yayınlı katalogda hosts eşlemesi hazırlanır ve localhost override kaldırılır; origin imzalı metadata control URL'leriyle aynı olmalıdır. [Yerel Caddy ve VM hazırlığı](../../deploy/README.md#yerel-hyper-v-https).
