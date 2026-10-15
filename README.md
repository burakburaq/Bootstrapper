# TalayLabs Installer

Ortak Windows web installer, update-client/protocol, update sunucusu ve release araçları. Owllig Academy ve Campus ürün ayarlarını kendi repolarında tutar; installer kodunu kopyalamaz. Ürünler test edilmiş installer commit SHA'sına bağlanır.

## Ürün derleme

Node.js 24+, Rust 1.97.1 MSVC, Windows SDK / Visual C++ Build Tools gerekir.

```powershell
npm ci
npm run compile -- --product ../owlligAcademy/installer/product.json --trust ../owlligAcademy/installer/trust.template.json
```

`trust.template.json` kurulum yeteneği kapalı kaynak şablonudur. Gerçek teslimde imzalı Windows politikası, public güven anahtarları ve son imzalı helper'ın HTTPS adres/hash/boyut bilgilerini içeren ayrı release trust dosyası verilmelidir. Özel anahtarlar ürün ayarlarına, EXE'ye veya Git'e girmez. Üretim TLS doğrulaması kapatılamaz.

Ürün JSON alanları: `schemaVersion: 1`, `appId`, `displayName`, `publisher`, `productDirectory`, `executable`, PNG/WebP `icon` yolu ve `description`. İkon yolu JSON dosyasına göredir. Derleme arayüz için 256 px WebP ve EXE için ICO üretir. `productDirectory` mevcut Windows sözleşmesinde Program Files altında tek güvenli klasör bileşenidir; iç içe/absolute/UNC yollar kabul edilmez. `publisher` görünen isimdir; yayıncı sertifika pini ayrı imzalı trust/policy alanıdır.

`build/` içindeki ürün/native/UI/Tauri ayarları ve `package-inputs.json` üretilir; kaynaklar değiştirilmez. Aynı checkout'ta ürün derlemeleri sırayla çalıştırılır. Paralel CI işlerindeki checkout'lar bağımsızdır. Doğrudan Cargo kullanmadan önce `npm run build` çalıştırılmalıdır; ürün EXE'si için `compile` komutu generated Tauri override'ını da uygular.

```powershell
npm run check
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml --locked
cargo clippy --manifest-path src-tauri/Cargo.toml --locked --all-targets -- -D warnings
```

Çıktı `src-tauri/target/release/talaylabs-installer.exe` olur. Bu derleme imzasızdır. Kullanıcı teslimi: final helper'ı imzala ve trust'a bağla → ürün installer'ını derle → UPX sıkıştır → Authenticode imzala → nihai SHA-256/boyutu kaydet → Windows VM kabulü → aynı EXE'yi yayımla. İmza/hash sonrasında dosya değiştirilmez.

## Uygulama paketine bağlantı

Installer'ın son imzalı kopyası `maintenance.exe` olarak payload'a eklenebilir. `build/package-inputs.json` uygulama kimliğini, ürün klasörünü, ana EXE'yi ve görünen ürün/yayıncı adlarını NSIS üretimiyle eşleştirmek içindir. `release-cli windows-package` final payload envanterini hazırlar; NSIS EXE ayrıca Authenticode ve updater imzası alır. `prepare → verify → upload → draft → publish` sırası [updates/README.md](updates/README.md) içinde açıklanır. Helper mevcut kurulum üzerine otomatik yazılmaz; helper değişikliği ayrı servis bakım dağıtımı gerektirir.

## CI ve private repo erişimi

PR'da UI ve ilgili native/shared update testleri çalışır; yeni PR commit'i eski koşuyu iptal eder. Push otomatik yayın başlatmaz. Ürün reposundaki manuel `product-installer.yml`, seçilen main SHA'sını ve `engine.json` içindeki sabit installer SHA'sını checkout edip ortak `compile` komutuyla imzasız cloud artifact üretir.

Private repo checkout ve Cargo Git bağımlılığı için uygulama reposunda `INSTALLER_READ_TOKEN` gerekir: yalnız `TalayLabs/installer` reposuna Contents: read yetkili fine-grained token. Kaynak kodda saklanmaz; Actions secret olarak tanımlanır. Deploy key veya organizasyon geneline reusable workflow paylaşımı gerekmez. Bu read yetkisi paket upload/publish veya imzalama yetkisi değildir. Cargo SDK bağımlılığı ile `engine.json` aynı test edilmiş commit'i sabitler. Token süresi dolmadan yenilenip her ürünün secret'ı güncellenmelidir.


Üretim imzalama/paketleme/publish ayrı, manuel ve yetkili release aşamasıdır. Bir installer yükseltmesi kurulu uygulamayı otomatik güncellemez; bakım arayüzü uygulama paketi üzerinden güncellenir.

Eski BaykuşLig installer teslim ve VM kayıtları [tarihsel belgede](docs/history/baykuslig-installer.md) korunur; eski absolute yollar ve komutlar yeni repo için güncel talimat değildir.

NSIS ürün hazırlığı: `npm run package:product -- --payload <final-signed-payload> --version <version> --publisher-certificate-sha256 <pin> --maintenance-executable maintenance.exe --out <output>`. Son ürün ayarları kullanılır; makensis ve imzalama ayrı adımlardır.
