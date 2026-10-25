# Installer

Windows uygulamaları için yapılandırılabilir kurulum ve güncelleme altyapısı. Tauri tabanlı kurulum arayüzünü, Rust güncelleme istemcisini, güncelleme sunucusunu ve sürüm yayınlama araçlarını bir araya getirir.

## Özellikler

- Ürün kimliği, uygulama adı, yayıncı, ikon ve açıklama ile özelleştirilebilir kurulum arayüzü.
- İmzalı metadata, paket imzası, SHA-256 ve boyut doğrulaması.
- Tam paket ve delta güncelleme desteği.
- Windows kurulum ve bakım işlemleri için ayrı servis bileşeni.
- Kanal bazlı yayın, kademeli dağıtım ve yayın yaşam döngüsü yönetimi.
- PostgreSQL destekli güncelleme kataloğu ve Cloudflare R2 üzerinden paket dağıtımı.
- Paket hazırlama, doğrulama, yükleme ve yayınlama için komut satırı araçları.

## Bileşenler

| Bileşen | Görevi |
| --- | --- |
| Installer | Tauri tabanlı Windows kurulum ve bakım arayüzü |
| Update Protocol | Metadata şemaları, imza kuralları ve yayın sözleşmeleri |
| Update Client | Paket indirme, doğrulama ve güncelleme işlemleri |
| Update Server | Güncelleme API'si, yayın kataloğu ve paket yönlendirmesi |
| Release CLI | Paket hazırlama, imzalı metadata üretimi ve yayın yönetimi |

## Gereksinimler

Installer derlemek için:

- Windows
- Node.js 24 veya üzeri
- Rust 1.97.1 ve MSVC araç zinciri
- Visual C++ Build Tools ve Windows SDK

Güncelleme sunucusu PostgreSQL kullanır. R2 üzerinden dağıtım için bir Cloudflare R2 bucket'ı ve uygun erişim yetkileri gerekir. NSIS paket üretimi için ayrıca NSIS kurulmalıdır.

## Başlangıç

Bağımlılıkları yükleyin:

```powershell
npm ci
```

Installer, ürün yapılandırması ve güven yapılandırması ile derlenir:

```powershell
npm run compile -- --product <product-config> --trust <trust-config>
```

`<product-config>` ve `<trust-config>` yer tutucularını kendi yapılandırmalarınızla değiştirin.

Ürün yapılandırması uygulama kimliğini, görünen adı, yayıncıyı, telif bildirimini, kurulum klasörü adını, ana çalıştırılabilir dosyayı, ikonu ve açıklamayı tanımlar. Güven yapılandırması public anahtarları, imzalı Windows politikasını ve kurulum yardımcısının doğrulama bilgilerini içerir.

Derleme tek başına dağıtıma hazır, imzalı bir installer üretmez. Kullanıcılara sunulacak çıktılar için imzalama ve yayın adımları ayrıca tamamlanmalıdır.

## Geliştirme kontrolleri

Arayüz ve derleme araçlarını kontrol etmek için:

```powershell
npm run check
```

Native installer bileşenini kontrol etmek için:

```powershell
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml --locked
cargo clippy --manifest-path src-tauri/Cargo.toml --locked --all-targets -- -D warnings
```

Güncelleme altyapısını kontrol etmek için:

```powershell
cargo test --manifest-path updates/Cargo.toml --workspace --locked
```

## Yayın akışı

1. Uygulama dosyalarını ve kurulum yardımcısını hazırlayın, gerekli imzaları uygulayın.
2. Installer'ı ürün ve güven yapılandırmasıyla derleyip nihai çıktıyı imzalayın.
3. Release CLI ile paketleri ve imzalı yayın metadata'sını hazırlayın.
4. Paketlerin imza, hash ve boyut bilgilerini doğrulayın.
5. Paketleri dağıtım depolamasına yükleyin ve sunucuda taslak yayın oluşturun.
6. Kabul kontrollerinden sonra yayını etkinleştirin.

Tam paket temel dağıtım biçimidir. Uygun bir önceki paket verildiğinde, delta tam paketten küçükse delta çıktısı da oluşturulur. Kademeli dağıtım politikaları yayın öncesinde ayarlanabilir.

İmzalama tamamlandıktan sonra paket içeriği değiştirilmemelidir. Doğrulanan ve kabul edilen dosyalar aynı içerikle yayımlanmalıdır.

## Güven modeli

Metadata imza anahtarları, uygulama paketi imza anahtarları ve Windows yayıncı sertifikası ayrı sorumluluklara sahiptir. İstemcinin güvendiği public anahtarlar önceden yapılandırılır; sunucu yanıtı yeni bir güven anahtarı kaynağı olarak kullanılmaz.

Özel anahtarlar ve erişim token'ları kaynak koda veya dağıtılan installer'a eklenmemelidir. Paket yükleme ve yayın yönetimi yetkileri ayrı tutulmalıdır. Üretim bağlantılarında HTTPS ve TLS sertifika doğrulaması zorunludur.

## Lisans

Bu proje MIT Lisansı ile lisanslanmıştır. Ayrıntılar için [LICENSE](LICENSE) dosyasına bakın.
