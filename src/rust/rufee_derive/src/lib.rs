use proc_macro::TokenStream;
use quote::quote;
use syn;

#[proc_macro_derive(HelloWorld)]
pub fn hello_world_derive(input: TokenStream) -> TokenStream {
    // 解析传入的 Rust 代码为 AST (抽象语法树)
    let ast = syn::parse(input).unwrap();

    // 构建实现代码
    impl_hello_world(&ast)
}

fn impl_hello_world(ast: &syn::DeriveInput) -> TokenStream {
    let name = &ast.ident;
    let gen = quote! {
        #[pymethods]
        impl #name {
            fn hello_world(&self) {
                println!("Hello, World! I am a {}!", stringify!(#name));
            }

            fn hello_there(&self) {
                println!("Hello there! I am a {}!", stringify!(#name));
            }
        }
    };
    gen.into()
}

#[proc_macro_derive(Node)]
pub fn node_derive(input: TokenStream) -> TokenStream {
    let ast = syn::parse(input).unwrap();

    impl_node(&ast)
}

fn impl_node(ast: &syn::DeriveInput) -> TokenStream {
    let name = &ast.ident;
    let data = &ast.data;
    println!("{:?}", data);

    let mut clone_impl = quote! {};

    match &ast.data {
        syn::Data::Struct(syn::DataStruct { fields, .. }) => {
            for field in fields {
                let identifier = field.ident.as_ref().unwrap();
                clone_impl.extend(quote! {
                    #identifier: self.#identifier.clone(),
                });

                let expanded = match fields {
                    syn::Fields::Named(mut fields_named) => {
                        // 为 named fields 的 struct 增加 coerce 字段
                        fields_named.named.push(syn::Field::parse_named(
                            syn::parse(quote! {
                                pub coerce: bool
                            })
                            .unwrap(),
                        ));

                        quote! {
                            struct #name {
                                #fields_named
                            }
                        }
                    }
                    _ => {
                        panic!("AddCoerce only supports named fields")
                    }
                };
            }
        }
        _ => unimplemented!(),
    }

    quote! {
        #[pymethods]
        impl #name {
            // fn clone(&self) -> Self {
            //     Self {
            //         #clone_impl
            //     }
            // }

            // fn coerce(&self, _coerce: bool) -> Self {
            //     if _coerce != self._coerce {
            //         let mut clone = self.clone();
            //         clone._coerce = _coerce;
            //         clone
            //     }
            //     self.clone()
            // }

            // fn is_alias(&self) -> bool {
            //     false
            // }

            // fn unwrap(&self) -> Self {
            //     // self.clone()
            //     self
            // }
        }
    }
    .into()
}
