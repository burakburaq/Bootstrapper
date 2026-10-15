# Yapılandırılmış release-cli

Ürün reposunda iki yapılandırma dosyası bulunur:

- `installer/product.json`: uygulama/updater kimliği, görünen isim, yayıncı, ürün klasörü, ana EXE, ikon ve açıklama. Installer derlemesinde kullanılan dosyanın aynısıdır.
- `release/config.json`: yayın ortamı, kanal/hedef, final paket ve imza yolları, immutable çıktı klasörü ve metadata key ID. Private key/token/secret değerleri bu dosyada kabul edilmez.

Ürün reposundaki `installer/engine.json`, installer ve yayın aracını aynı tam commit SHA'sına sabitler. `scripts/release.ps1` bu kaynağı ignored `.cache` içine checkout eder; farklı SHA veya değişmiş kaynakla çalışmayı reddeder. Kaynak kod uygulama reposuna kopyalanmaz.

Örnek `release/config.json`:

```json
{
  "schemaVersion": 1,
  "protocol": "talaylabs-v1",
  "serverUrl": null,
  "bucket": null,
  "channel": "production",
  "target": "windows-x86_64-nsis",
  "package": "../.cache/packages/{version}/package.exe",
  "packageSignature": "../.cache/packages/{version}/package.exe.sig",
  "installationContract": "../.cache/packages/{version}/contract.json",
  "output": "../.cache/releases/{version}",
  "metadataKeyId": "release"
}
```

`serverUrl` update API'sinin HTTPS origin'idir; path, query, kullanıcı adı/parola kabul edilmez. `bucket` R2 bucket adıdır. Bunlar `null` iken offline yapılandırma kontrolü/hazırlık yapılabilir; gerçek upload/draft/publish öncesi doldurulmalıdır. `validate` bunu `publishingConfigured: false` olarak açıkça bildirir. Varsayılan URL/bucket uydurulmaz, mevcut deployment değiştirilmez.

Yollar `release/config.json` dosyasının bulunduğu klasöre göre çözülür. `icon`, `product.json` dosyasına göredir. Yalnız `{version}` ve `{appId}` şablonları desteklenir; shell expansion yapılmaz. `--package`, `--output`, `--base` geçersiz kılmaları çağıranın çalışma dizinine göredir. `--base`, daha önce yayımlanmış final full pakettir; delta dönüşümü byte olarak kontrol edilir. `target`, installer/helper politikasındaki hedefle aynı olmalıdır; mevcut Windows NSIS akışı `windows-x86_64-nsis` kullanır.

`metadataKeyId`, güvenilir metadata public key haritasındaki gerçek ID ile eşleşmelidir. `release` yalnız başlangıç ayarıdır; mevcut dağıtımın anahtar ID'sini kullanın. Sürüm ve artan `sequence` her yayın için ayrıca verilir; `schemaVersion` uygulama sürümü değildir.

## Komutlar

Ürün kökünde:

```powershell
./scripts/release.ps1 -Action validate
./scripts/release.ps1 -Action prepare -Version 0.1.0 -Sequence 1
./scripts/release.ps1 -Action verify -Version 0.1.0
./scripts/release.ps1 -Action upload -Version 0.1.0
./scripts/release.ps1 -Action draft -Version 0.1.0
./scripts/release.ps1 -Action publish -Version 0.1.0
```

Doğrudan ortak CLI:

```powershell
cargo run --manifest-path updates/Cargo.toml -p release-cli --locked -- project --product <product.json> --config <config.json> --version 0.1.0 --sequence 1 prepare
```

`validate` secret okumaz, paket aramaz, build/yükleme/yayın yapmaz. Diğer aşamalar için sürüm gerekir; native `prepare` ayrıca sequence ister. Varsayılan PowerShell eylemi `validate`'dir; publish otomatik çağrılmaz.

`prepare` hazır, final imzalı paketi tüketir. Uygulamayı yeniden derlemez, Authenticode/updater imzası atmaz. NSIS üretimi, Authenticode imzası ve updater paketi imzası önceki build/sign adımlarıdır. `contract.json`, `windows-package` tarafından final imzalı payload'dan üretilir. Yalnız updater dağıtımı yapan ve kurulum sözleşmesi istemeyen ürünler `installationContract` alanını null bırakabilir.

`verify`, imzalı metadata yanında hazırlanmış full/delta dosyalarının gerçek hash ve boyutlarını denetler. Ürün/kanal/hedef/sürüm karışırsa işlem durur. `upload` R2'ye koşullu PUT yapar; mevcut obje aynı içerikse doğrulanır, farklı içerikse reddedilir. `draft` upload makbuzunu, `publish` draft makbuzunu ister. Makbuzlar bundle hash'ine, ürüne, sunucuya ve bucket'a bağlıdır; config veya bundle değiştirilirse yeniden ilgili aşama gerekir.

Aynı çıktı üzerinde paralel uzak işlemler `operation.lock` ile engellenir. Crash sonrası kalan kilidi yalnız o işlemin artık çalışmadığını doğruladıktan sonra elle kaldırın. Hazırlanmış çıktı dizini üzerine yazılmaz. Uzak draft oluşturma sırasında yarım kalan bir API işlemi otomatik olarak geri alınmaz; sunucudaki draft durumu incelenmelidir. Kaynak/paket/hash değiştirerek var olan bir release sürümünü tekrar kullanmayın.

## Secret ve public key yolları

| Ortam değişkeni | İçerik |
| --- | --- |
| `RELEASE_METADATA_KEY_FILE` | 32-byte hex metadata private key dosyasının yolu; yalnız prepare gerekir |
| `RELEASE_METADATA_PUBLIC_KEY_FILE` | Güvenilir `{ "key-id": "base64-public-key" }` JSON dosyasının yolu |
| `RELEASE_PACKAGE_PUBLIC_KEY_FILE` | Final updater paketini doğrulayan public key dosyasının yolu; prepare gerekir |
| `R2_ENDPOINT` | Cloudflare R2 S3 HTTPS endpoint'i; upload gerekir |
| `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY` | R2 yazma credentials; upload gerekir |
| `UPDATE_ADMIN_TOKEN` | Katalog yönetim token'ı; draft/publish gerekir |

R2/token değerleri için doğrudan değişken yerine `_FILE` varyantı kullanılabilir; dosyanın içeriği okunur. Örneğin `R2_SECRET_ACCESS_KEY_FILE` ve `UPDATE_ADMIN_TOKEN_FILE`. Private/public key değişkenlerinin kendileri zaten dosya yollarıdır; ekstra `_FILE` eklenmez. Anahtarlar çıktı bundle'ına veya komut satırına yazılmaz. Yerel HTTPS test CA'sı gerekiyorsa `-CaCertificate` açıkça verilmelidir; TLS doğrulaması kapatılmaz.

Release adımları mevcut manuel cloud pipeline'da seçilen ürün SHA'sının derleme/sign/test çıktıları üzerinde çalıştırılır. Test edilmiş paket tekrar derlenmeden aynı baytlarla upload edilir. Bu entegrasyon kendiliğinden bir GitHub yayın workflow'u veya yeni R2 sürümü oluşturmaz.
