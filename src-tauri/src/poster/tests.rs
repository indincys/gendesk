#![allow(clippy::unwrap_used, clippy::expect_used)] // 隔离库与临时文件的行为断言。
use super::*;

fn image(sku: &str, style: &str) -> Image {
    Image {
        canonical_path: None,
        uid: "abcd".into(),
        item_name: "场景图".into(),
        sku: sku.into(),
        style: style.into(),
        image_path: String::new(),
        product_id: None,
        product_name: None,
    }
}

#[test]
fn twenty_styles_match_exactly_and_never_guess_numbers_or_conflicting_artists() {
    let products = ARTISTS
        .iter()
        .enumerate()
        .map(|(i, (artist, _))| client::PosterProduct {
            id: format!("product-{i}"),
            name: format!("{artist}日历（2027）"),
            is_generic: false,
        })
        .collect::<Vec<_>>();
    let images = ARTISTS
        .iter()
        .map(|(artist, _)| image(&format!("{artist}日历"), artist))
        .collect::<Vec<_>>();
    let result = groups(&images, &products, &[]);
    assert_eq!(result.len(), 20);
    for (index, group) in result.iter().enumerate() {
        assert_eq!(group.product_id, Some(format!("product-{index}")));
    }
    let unknown = groups(
        &[image("55", "王一博"), image("王俊凯日历", "蔡徐坤")],
        &products,
        &[],
    );
    assert!(unknown
        .iter()
        .all(|g| g.product_id.is_none() && !g.warning.is_empty()));
    assert!(identity("王俊凯日历", "蔡徐坤").is_err());
    let renamed = vec![client::PosterProduct {
        id: "fixed-id".into(),
        name: "用户改过显示名".into(),
        is_generic: false,
    }];
    let binding = Binding {
        sku: "王俊凯日历".into(),
        style: "王俊凯".into(),
        product_id: "fixed-id".into(),
    };
    assert_eq!(
        groups(&[image("王俊凯日历", "王俊凯")], &renamed, &[binding])[0]
            .product_id
            .as_deref(),
        Some("fixed-id")
    );
}

async fn seed_batch(pool: &SqlitePool, root: &Path, id: &str) {
    begin_acceptance(pool, id).await.unwrap();
    let path = pending_path(root, id, "abcd", "png");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    image::RgbImage::from_pixel(2, 2, image::Rgb([90, 100, 110]))
        .save(&path)
        .unwrap();
    sqlx::query("INSERT INTO poster_images(batch_id,work_id,uid,item_name,sku,style,image_path) VALUES(?,1,'abcd','王俊凯日历_书桌晒单','王俊凯日历','王俊凯',?)")
        .bind(id).bind(path.to_string_lossy().as_ref()).execute(pool).await.unwrap();
}

#[tokio::test]
async fn optional_integration_defaults_off_and_does_not_track_reviews() {
    let (pool, root) = crate::db::test_support::test_pool().await;
    assert!(!load_settings(&pool, root.path()).await.unwrap().enabled);
    // 升级前只有应用路径与图片目录的旧设置也必须默认关闭。
    crate::db::repo::settings::set_by_key(
        &pool,
        "poster",
        r#"{"executable":"/missing/Poster","libraryRoot":"/unused/images"}"#,
    )
    .await
    .unwrap();
    assert!(!load_settings(&pool, root.path()).await.unwrap().enabled);
    let id = new_id();
    assert!(!begin_review(&pool, root.path(), &id).await.unwrap());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM poster_batches")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert!(!root.path().join("Poster待确认").exists());
    assert!(preview(&pool, root.path(), &id).await.is_err());
    assert!(confirm(&pool, root.path(), &id, &[]).await.is_err());
    let settings = PosterSettings {
        enabled: true,
        executable: "/missing/Poster".into(),
        library_root: root.path().join("images").to_string_lossy().into(),
    };
    crate::db::repo::settings::set_by_key(
        &pool,
        "poster",
        &serde_json::to_string(&settings).unwrap(),
    )
    .await
    .unwrap();
    assert!(begin_review(&pool, root.path(), &id).await.unwrap());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM poster_batches")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[cfg(unix)]
#[tokio::test]
async fn disabling_pauses_confirmed_delivery_until_reenabled() {
    let (pool, root) = crate::db::test_support::test_pool().await;
    initialize(&pool).await.unwrap();
    let id = new_id();
    seed_batch(&pool, root.path(), &id).await;
    finish_acceptance(&pool, &id).await.unwrap();
    let mut settings = PosterSettings {
        enabled: true,
        executable: fake_poster(root.path()).to_string_lossy().into(),
        library_root: root.path().join("permanent").to_string_lossy().into(),
    };
    crate::db::repo::settings::set_by_key(
        &pool,
        "poster",
        &serde_json::to_string(&settings).unwrap(),
    )
    .await
    .unwrap();
    let group = preview(&pool, root.path(), &id)
        .await
        .unwrap()
        .groups
        .remove(0);
    confirm(
        &pool,
        root.path(),
        &id,
        &[PosterChoice {
            key: group.key,
            product_id: "fixed-id".into(),
        }],
    )
    .await
    .unwrap();
    settings.enabled = false;
    crate::db::repo::settings::set_by_key(
        &pool,
        "poster",
        &serde_json::to_string(&settings).unwrap(),
    )
    .await
    .unwrap();
    deliver_pending(&pool, root.path()).await.unwrap();
    assert_eq!(batches(&pool).await.unwrap()[0].state, "queued");
    assert!(!Path::new(&settings.library_root).exists());
    let request: Option<String> =
        sqlx::query_scalar("SELECT request_json FROM poster_batches WHERE id=?")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(request.is_none());
    settings.enabled = true;
    crate::db::repo::settings::set_by_key(
        &pool,
        "poster",
        &serde_json::to_string(&settings).unwrap(),
    )
    .await
    .unwrap();
    deliver_pending(&pool, root.path()).await.unwrap();
    assert_eq!(batches(&pool).await.unwrap()[0].state, "done");
}

#[tokio::test]
async fn restart_does_not_turn_unconfirmed_or_skipped_batches_into_imports() {
    let (pool, root) = crate::db::test_support::test_pool().await;
    initialize(&pool).await.unwrap();
    let id = new_id();
    seed_batch(&pool, root.path(), &id).await;
    initialize(&pool).await.unwrap();
    assert_eq!(batches(&pool).await.unwrap()[0].state, "awaiting");
    deliver_pending(&pool, root.path()).await.unwrap();
    assert_eq!(batches(&pool).await.unwrap()[0].state, "awaiting");
    skip(&pool, root.path(), &id).await.unwrap();
    initialize(&pool).await.unwrap();
    deliver_pending(&pool, root.path()).await.unwrap();
    assert_eq!(batches(&pool).await.unwrap()[0].state, "skipped");
    assert!(!root.path().join("Poster待确认").join(&id).exists());
    assert!(!root.path().join("Poster图片库").exists());
}

#[cfg(unix)]
fn fake_poster(root: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join("Poster");
    std::fs::write(&path, r##"#!/usr/bin/env python3
import sys,json,pathlib,hashlib
if sys.argv[1]=='export-image-products':
    print(json.dumps({'schema':'poster.images/1','products':[{'id':'fixed-id','name':'王俊凯日历（2027）'}]}))
else:
    data=json.loads(pathlib.Path(sys.argv[2]).read_text())
    for item in data['items']:
        assert hashlib.sha256(pathlib.Path(item['path']).read_bytes()).hexdigest()==item['sha256']
    if pathlib.Path(__file__).with_name('fail-once').exists():
        pathlib.Path(__file__).with_name('fail-once').unlink()
        sys.exit(7)
    data['applied']=True
    data['items']=[{'externalId':x['externalId'],'productId':x['productId'],'imageId':'image-'+x['externalId'],'status':'imported'} for x in data['items']]
    print(json.dumps(data))
"##).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
#[tokio::test]
async fn confirmed_batch_keeps_frozen_paths_across_failure_settings_change_and_retry() {
    let (pool, root) = crate::db::test_support::test_pool().await;
    initialize(&pool).await.unwrap();
    let id = new_id();
    seed_batch(&pool, root.path(), &id).await;
    finish_acceptance(&pool, &id).await.unwrap();
    let executable = fake_poster(root.path());
    let mut settings = PosterSettings {
        enabled: true,
        executable: executable.to_string_lossy().into(),
        library_root: root.path().join("永久图片").to_string_lossy().into(),
    };
    crate::db::repo::settings::set_by_key(
        &pool,
        "poster",
        &serde_json::to_string(&settings).unwrap(),
    )
    .await
    .unwrap();
    let group = preview(&pool, root.path(), &id)
        .await
        .unwrap()
        .groups
        .remove(0);
    confirm(
        &pool,
        root.path(),
        &id,
        &[PosterChoice {
            key: group.key,
            product_id: "fixed-id".into(),
        }],
    )
    .await
    .unwrap();
    assert_eq!(batches(&pool).await.unwrap()[0].state, "queued");
    assert!(skip(&pool, root.path(), &id).await.is_err());
    std::fs::write(root.path().join("fail-once"), "").unwrap();
    deliver_pending(&pool, root.path()).await.unwrap();
    assert_eq!(batches(&pool).await.unwrap()[0].state, "failed");
    let json: String = sqlx::query_scalar("SELECT request_json FROM poster_batches WHERE id=?")
        .bind(&id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let request: client::Request = serde_json::from_str(&json).unwrap();
    assert!(Path::new(&request.items[0].path).starts_with(&settings.library_root));
    // 模拟重启前原验收缓存已不可读、用户已更改新批次的永久目录。
    cleanup_pending(root.path(), &id).await;
    settings.library_root = root.path().join("别的目录").to_string_lossy().into();
    crate::db::repo::settings::set_by_key(
        &pool,
        "poster",
        &serde_json::to_string(&settings).unwrap(),
    )
    .await
    .unwrap();
    initialize(&pool).await.unwrap();
    deliver_pending(&pool, root.path()).await.unwrap();
    assert_eq!(batches(&pool).await.unwrap()[0].state, "done");
    assert!(Path::new(&request.items[0].path).is_file());
    assert!(!Path::new(&settings.library_root).exists());
}

#[test]
fn permanent_files_are_reused_and_conflicts_are_never_overwritten() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.png");
    image::RgbImage::from_pixel(2, 2, image::Rgb([1, 2, 3]))
        .save(&source)
        .unwrap();
    let mut image = image("王俊凯日历", "王俊凯");
    image.image_path = source.to_string_lossy().into();
    image.product_id = Some("p".into());
    image.product_name = Some("王俊凯日历（2027）".into());
    let library = root.path().join("library");
    let batch = new_id();
    let a = storage::prepare(
        library.to_str().unwrap(),
        &batch,
        "source".into(),
        &[image.clone()],
    )
    .unwrap();
    let b = storage::prepare(
        library.to_str().unwrap(),
        &batch,
        "source".into(),
        &[image.clone()],
    )
    .unwrap();
    assert_eq!(a.items[0].path, b.items[0].path);
    std::fs::write(&a.items[0].path, b"changed").unwrap();
    assert!(
        storage::prepare(library.to_str().unwrap(), &batch, "source".into(), &[image]).is_err()
    );
    assert_eq!(std::fs::read(&a.items[0].path).unwrap(), b"changed");
}

/// 可选的跨应用联调，必须显式提供隔离数据目录与已绑定隔离 Poster 库的 CLI 包装器。
#[tokio::test]
#[ignore = "需显式提供隔离的 Poster CLI 与数据目录"]
async fn real_poster_cli_roundtrip() {
    let root = PathBuf::from(std::env::var("GENDESK_POSTER_E2E_DATA_DIR").expect("需隔离数据目录"));
    assert!(root.is_absolute() && root.to_string_lossy().contains("_草稿/开发验证/"));
    std::fs::create_dir_all(&root).unwrap();
    let pool = crate::db::connect(&root.join("gendesk.db")).await.unwrap();
    initialize(&pool).await.unwrap();
    let settings = PosterSettings {
        enabled: true,
        executable: std::env::var("GENDESK_POSTER_E2E_BIN").expect("需隔离 CLI 包装器"),
        library_root: root.join("成品图片").to_string_lossy().into(),
    };
    crate::db::repo::settings::set_by_key(
        &pool,
        "poster",
        &serde_json::to_string(&settings).unwrap(),
    )
    .await
    .unwrap();
    let id = new_id();
    seed_batch(&pool, &root, &id).await;
    finish_acceptance(&pool, &id).await.unwrap();
    deliver_pending(&pool, &root).await.unwrap();
    assert_eq!(
        batches(&pool)
            .await
            .unwrap()
            .iter()
            .find(|b| b.id == id)
            .unwrap()
            .state,
        "awaiting"
    );
    let preview = preview(&pool, &root, &id).await.unwrap();
    let group = &preview.groups[0];
    let product_id = group.product_id.clone().expect("真实目录应有王俊凯日历款");
    confirm(
        &pool,
        &root,
        &id,
        &[PosterChoice {
            key: group.key.clone(),
            product_id,
        }],
    )
    .await
    .unwrap();
    deliver_pending(&pool, &root).await.unwrap();
    let batch = batches(&pool)
        .await
        .unwrap()
        .into_iter()
        .find(|b| b.id == id)
        .unwrap();
    assert_eq!(batch.state, "done", "{}", batch.last_error);
    let receipt: String = sqlx::query_scalar("SELECT receipt_json FROM poster_batches WHERE id=?")
        .bind(&id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let receipt: client::Receipt = serde_json::from_str(&receipt).unwrap();
    assert_eq!(receipt.items.len(), 1);
    assert_eq!(receipt.items[0].status, "imported");
    // 保留隔离库与永久文件，供真实 WebView 确认流程的人工验收继续使用。
}

#[test]
fn explicit_generic_products_without_legacy_sku_do_not_merge_poster_selection_groups() {
    let a = crate::library::Organization::legacy("陶瓷水杯", "奶油白款", "厨房场景");
    let b = crate::library::Organization::legacy("玻璃水杯", "奶油白款", "厨房场景");
    let (as_, at) = a.poster_keys("", "");
    let (bs, bt) = b.poster_keys("", "");
    let result = groups(&[image(&as_, &at), image(&bs, &bt)], &[], &[]);
    assert_eq!(result.len(), 2);
    assert_ne!(result[0].key, result[1].key);
    assert_eq!(
        a.poster_keys("旧SKU", "原款式"),
        ("旧SKU".into(), "原款式".into())
    );
}
