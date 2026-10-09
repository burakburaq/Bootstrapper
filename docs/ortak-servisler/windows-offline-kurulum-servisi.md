# Windows Çevrimdışı Kurulum Servisi

Bu belge ortak update-client'ın Windows kurulum servisi için kabul edilmiş tasarım sınırlarını tanımlar. Karar: [DEC-C056](../baykuslig/kararlar/kapatilan-kararlar-c056.md). Baykuş Lig'in ürün/kurulum politikası [ürün belgesindedir](../baykuslig/urun/guncellemeler-ve-platform-destegi.md). Ortak release/API sözleşmesi [update server belgesinde](./update-server.md) kalır.

**Durum (8 Ekim 2026):** Windows servis EXE'si, yerel IPC, envanterli stage/activate NSIS üretimi ve offline kurulum motoru ortak update-client içinde uygulandı. Dört crate düzeni korunur. App/installer bağlaması, gerçek imzalı paket/servis dağıtımı ve Windows VM kabulü açıktır. Servis ayrı Windows prosesi olarak update-client paketinin Windows bileşeninden üretilir; VPS'teki update-server değildir. [Kod kanıtı ve açık kabul](../planlama/dogrulama/update-client-installer.md).

Installer/uninstaller ile ortak ürün kimliği, dosya alanları, UI köprüsü, kilit ve kullanıcı verisi sınırları [Windows kurulum sözleşmesi v1](./windows-kurulum-sozlesmesi.md) ile tanımlanır. [DEC-C058](../baykuslig/kararlar/kapatilan-kararlar-c058.md) yerel web ekranı/native fallback seçimini kaydeder; servisin çevrimdışı doğrulama kurallarını değiştirmez.

## Görev ayrımı

| İş | Uygulama / normal yetkili update-client | Yerel Windows servisi |
|---|---|---|
| Sürüm, rollout ve güncel lifecycle kontrolü | Çevrimiçi olarak yapar | Ağ isteği yapmaz |
| Full/delta indirme, URL yenileme, full fallback | Yapar | Yapmaz |
| Delta'dan tam paketi hazırlama | Yapar | Yalnız hazırlanmış tam paketi kabul eder |
| Kullanıcı bildirimi, kurulum onayı ve güvenli çıkış | Yapar | UI göstermez |
| İmzalı metadata ve tam paket doğrulaması | Hazırlama sırasında yapar | Kurulumdan önce bağımsız olarak tekrar yapar |
| Program Files kurulumu ve kalıcı kurulum kaydı | Yetkili servise başvurur | Yapar |
| Kullanıcı oturumunda yeniden açılma | Normal yetkiyle yapar | Kullanıcı uygulamasını yönetici/SYSTEM olarak başlatmaz |

Servis update-server, R2 veya başka bir sunucuya bağlanmaz; HTTP istemcisi, signed URL, upload/indirme tokenı ve private imza anahtarı taşımaz. İstek dosya ve imzalı metadata ile gelir; uygulamanın “doğruladım” sonucu servisin doğrulaması yerine geçmez. Installer ve servis doğrulama alt işlemleri de ağdan bileşen çekemez; gereken bağımlılıklar önceden paketlenmiş veya uygulama tarafından hazırlanmış olmalıdır.

## Kurulum ve kalıcı alan

İlk servis bootstrap'ı yönetici onayıyla yapılır ve servisi Windows Service Control Manager'a kaydeder. Sonraki normal güncellemeler yetkili servis üzerinden uygulanır, tekrar UAC sorusu çıkarmaz. Uygulama içindeki kurulum onayı korunur. Uygulanan servis LocalSystem hesabında, imzalı sabit ürün politikası ve dar pipe API'siyle çalışır; gereksiz genel yönetim işlevi sunulmaz. Gerçek bootstrap/sonraki UAC davranışı VM kabulünde doğrulanacaktır.

- Program ve servis EXE'leri Program Files altında normal kullanıcının değiştiremediği kurulum alanındadır.
- Kurulum kimliği, authoritative journal/sürüm-sequence tabanı ve servis staging alanı ProgramData altında uygulama/kurulum bazında tutulur. Normal kullanıcılar kurulum kimliğini ve kurulum durumunu değiştiremez.
- Kullanıcı ayarı/oturumu/önbelleği LocalAppData içindedir. Hesap değişimi veya başka bir Windows kullanıcısının giriş yapması ortak installation_id değerini değiştirmez.
- Dizin yolları ilk kurulumda güvenilir biçimde kaydedilir; çağıranın verdiği hedef yol yetkili yazma hedefi olarak kabul edilmez. Windows known-folder API'leri kullanılır, C: sürücüsü varsayımı kod içine gömülmez.

## Paket kabulü ve yerel iletişim

Yerel IPC yalnız izinli çağıranlara açıktır; Windows kimliği ve oturum kapsamı kontrol edilir. Remote pipe istemcileri reddedilir. API yalnız hazırlama/kurulum/durum gibi sınırlı işlemleri taşır; shell komutu, keyfi EXE, keyfi hedef dizin veya çağıranın installer argümanları kabul edilmez. Servisin kendi kayıtlı kurulum bağlamı app_id/channel/target, güven kökleri, ürün/yayıncı ve installer seçeneklerini belirler.

1. Gelen paket ve metadata boyut/sayı sınırlarıyla alınır. Kaynak dosya güvenli dosya handle'ı üzerinden okunur; UNC/uzak dosya ve reparse/symlink yönlendirmeleriyle başka hedefe yazılamaz.
2. Servis tam paketi kendisinin oluşturduğu, kısıtlı ACL taşıyan staging alanına alır. LocalAppData'daki dosya doğrulanıp daha sonra aynı kullanıcı-yazılabilir yoldan çalıştırılmaz. Çalıştırılan dosya doğrulanan korumalı kopyadır; servis staging içeriğine başka normal kullanıcı yazamaz.
3. Sabitlenmiş güven kökleriyle release manifesti ve artifact descriptor imzaları, şema, uygulama/kanal/hedef ve referans bağları doğrulanır. İstekten gelen public key kendiliğinden güven kökü olmaz.
4. Tam paketin byte boyutu, SHA-256 ve updater imzası kontrol edilir. Windows kod imzası/beklenen yayıncı-ürün, gerçek paket sürümü ve mimari ayrıca doğrulanır. Hash tek başına yayıncı doğrulaması değildir.
5. Servisin kalıcı sürüm/sequence tabanı ve gerçek kurulu sürüm üzerinden eski sürüme dönme, aynı sürümde farklı içerik ve checkpoint atlama reddedilir. Tamamlanmış isteğin tekrarı idempotent sonuç verir; yeniden installer çalıştırmaz. Yarım deneme başarı kabul edilmez ve kalıcı kayıtla uzlaştırılır.
6. Kuruluma başlamadan bütün ilgili kullanıcı oturumlarındaki uygulama süreçlerinin güvenli çıkışı doğrulanır. Çıkış tamamlanmıyorsa kurulum ertelenir; veri kaybına yol açacak zorla kapatma yapılmaz. Kurulum başına tek kilit eşzamanlı kullanıcı/servis isteklerini sıraya alır.
7. Niyet kalıcı journal'a yazılır; yalnız doğrulanmış installer sabit sessiz parametrelerle başlatılır. Exit code, kurulu ürün ve journal uzlaştırılır. Kullanıcı verileri korunur. Güncellenen servis bileşeninin stop/replace/start akışı da kesilmeye dayanıklı installer işlemi ve sonraki uzlaştırmayla kabul edilir.

Windows Authenticode doğrulamasında WinVerifyTrust ağdan sertifika/iptal bilgisi çekmemelidir; WTD_CACHE_ONLY_URL_RETRIEVAL dahil cache-only doğrulama ayarları kullanılır. Yerel güven kanıtı yeterli değilse doğrulama başarılı sayılmaz; online sorgu açılmaz. Sertifika zinciri ve zaman damgasının gerçekten çevrimdışı doğrulanabildiği temiz Windows kabulünde gösterilir.

## Güncel yayın izni sınırı

Uygulama, kullanıcı kurulumu onayladıktan sonra ve servise teslim etmeden hemen önce mevcut server lifecycle/kurulum iznini kontrol eder. Kontrol başarısızsa yeni kurulum gönderilmez; mevcut uygulama çalışmaya devam eder. Servisin çevrimdışı olması, uygulamanın bu çevrimiçi kontrolünün kaldırıldığı anlamına gelmez.

Offline paket imzası değişmez metadata ve dosya bütünlüğünü doğrular; sonradan değişen disabled/revoked/rollout durumunun güncel kanıtı değildir. Servis en son server durumunu kendi başına öğrenemez. Handoff'tan sonra yayın iptalinin anında öğrenileceği veya sınırsız süre beklemiş paketin güncel izinle kurulacağı garanti edilmez.

**8 Ekim uygulaması:** Normal uygulama kullanıcı onayından sonra servis `begin` challenge'ını `/v1/install/authorize` endpoint'ine iletir. Server güncel lifecycle/rollout/kanal yetkisini denetleyip ayrı operasyonel anahtarla nonce/işlem/kurulum kimliği/niyet/release/hash'e bağlı izin imzalar. Servis kendi monoton süresiyle tazeliği, paket alımından önce ve commit öncesinde bağları denetler; yeniden başlatmada yeni izin gerekir. Release metadata private key'i server'a verilmez. App bağlantısı, gerçek anahtar dağıtımı ve Windows kabulü tamamlanmadan güvenli ürün kurulumu kabul edilmiş sayılmaz.

## Kabul senaryoları

- İlk yönetici kurulumundan sonra normal kullanıcıyla NSIS güncellemesi; ikinci UAC ve elevated app prosesi yok.
- x86-64/ARM64 hedefleri; ağ kapalı servis altında başarılı doğrulama/kurulum, servis ve installer alt süreçlerinden ağ bağlantısı yok.
- Bozuk paket, değişmiş metadata, yanlış app/hedef/anahtar/yayıncı/sürüm; eksik imza/sertifika kanıtında kurulum yok.
- Dosya doğrulama/çalıştırma arasında değiştirme, kullanıcı-yazılabilir staging, symlink/reparse/UNC, keyfi komut/installer argümanı ve yetkisiz/remote IPC reddi.
- Aynı/daha eski paket tekrarı, çift kullanıcı isteği, açık diğer oturum, servis/installer/proses kesilmesi ve restart reconcile.
- Servisin kendisini içeren güncellemenin yarıda kesilmesi, servis kaydı ve protected state'in korunması.
- Uygulamanın son lifecycle kontrolünde revoke/server hatası; izin tazeliği mekanizmasının kendi kabul testleri.

Kaynaklar: [Windows servis yetkileri](https://learn.microsoft.com/en-us/windows/win32/services/service-security-and-access-rights), [named pipe erişimi](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights), [WinVerifyTrust cache-only ayarı](https://learn.microsoft.com/en-us/windows/win32/api/wintrust/ns-wintrust-wintrust_data).
