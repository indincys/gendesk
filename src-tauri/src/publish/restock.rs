//! 商品缺文案、SKU 缺图时的补料提示词。
//!
//! 文案已经上移到商品，因此模板只用 `【商品】`，图片需求仍精确到 SKU。

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductCopyNeed {
    pub code: String,
    pub name: String,
    pub title_free: usize,
    pub body_free: usize,
    pub title_target: usize,
    pub body_target: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkuImageNeed {
    pub product_code: String,
    pub code: String,
    pub name: String,
    pub image_free: usize,
    pub image_target: usize,
}

pub fn build_restock_prompt(copy: &[ProductCopyNeed], images: &[SkuImageNeed]) -> String {
    if copy.is_empty() && images.is_empty() {
        return String::new();
    }
    let mut out =
        String::from("你是电商内容运营。请按下面的缺口补齐素材，商品负责文案，SKU 负责图片。\n\n");
    if !copy.is_empty() {
        out.push_str("## 商品缺文案\n\n");
        for item in copy {
            let titles = item.title_target.saturating_sub(item.title_free);
            let bodies = item.body_target.saturating_sub(item.body_free);
            out.push_str(&format!(
                "- {} · {}：补标题 {} 条，补正文 {} 条\n",
                item.code, item.name, titles, bodies
            ));
        }
        out.push_str(
            "\n标题写入 `收件箱/{商品码}/标题.txt`，正文写入 `收件箱/{商品码}/正文.txt`。\n\
             文件头必须使用 `【商品】{商品码}`；标题一行一条，正文用单独一行 `====` 分隔。\n\n",
        );
    }
    if !images.is_empty() {
        out.push_str("## SKU 缺图\n\n");
        for item in images {
            out.push_str(&format!(
                "- 商品 {} · SKU {} · {}：现有 {} 张，再产出 {} 张\n",
                item.product_code,
                item.code,
                item.name,
                item.image_free,
                item.image_target.saturating_sub(item.image_free)
            ));
        }
        out.push_str(
            "\n每张图必须在生成前把提示词组绑定到对应 SKU，验收通过后会自动入图片素材库。\n",
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_uses_product_for_copy_and_sku_for_images() {
        let prompt = build_restock_prompt(
            &[ProductCopyNeed {
                code: "A".into(),
                name: "NFC 挂件".into(),
                title_free: 2,
                body_free: 1,
                title_target: 10,
                body_target: 5,
            }],
            &[SkuImageNeed {
                product_code: "A".into(),
                code: "A-STAR".into(),
                name: "黄星款".into(),
                image_free: 3,
                image_target: 10,
            }],
        );
        assert!(prompt.contains("补标题 8 条，补正文 4 条"));
        assert!(prompt.contains("【商品】{商品码}"));
        assert!(!prompt.contains("【SKU】"));
        assert!(prompt.contains("SKU A-STAR"));
        assert!(prompt.contains("再产出 7 张"));
    }

    #[test]
    fn empty_need_yields_empty_prompt() {
        assert!(build_restock_prompt(&[], &[]).is_empty());
    }
}
