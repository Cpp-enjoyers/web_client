
pub fn get_filename_from_path(s: &String) -> String{
    s.split('/').last().unwrap_or(s).to_string()
}

pub fn get_media_inside_html_file(file_str: &str) -> Vec<String> {
    let document = scraper::Html::parse_document(file_str);
    let mut ret = vec![];
    if let Ok(selector) = scraper::Selector::parse("img") {
        for path in document.select(&selector) {
            if let Some(path) = path.value().attr("src") {
                ret.push(path.to_string());
            }
        }
    }
    ret
}

#[cfg(test)]
mod utils_test {

    use crate::utils::{get_filename_from_path, get_media_inside_html_file};


    #[test]
    pub fn filename_from_path(){
        let path = "a/b/c/d/e/ciao.txt".to_string();
        assert_eq!(get_filename_from_path(&path), "ciao.txt".to_string());

        let path = "ciao.txt".to_string();
        assert_eq!(get_filename_from_path(&path), "ciao.txt".to_string());
    }

    #[test]
    fn file_parsing() {
        assert_eq!(
            get_media_inside_html_file(&"suhbefuiwfbwob".to_string()),
            Vec::<String>::new()
        );
        assert_eq!(
            get_media_inside_html_file(
                &"-----------<img src=\"youtube.com\"\\>".to_string()
            ),
            vec!["youtube.com".to_string()]
        );
        assert_eq!(
            get_media_inside_html_file(
                &"-----------<img src=\"/usr/tmp/folder/subfolder/pic.jpg\"\\>".to_string()
            ),
            vec!["/usr/tmp/folder/subfolder/pic.jpg".to_string()]
        );
    }
}