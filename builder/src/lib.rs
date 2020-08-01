#![allow(unused)]

extern crate proc_macro;

use std::collections::HashSet;

use proc_macro2::{Ident, Span, TokenStream};
use syn::{
    parse_macro_input, punctuated::Punctuated, Attribute, Data, DeriveInput, Field, Fields,
    FieldsNamed, Lit, Meta, NestedMeta, Path, PathArguments, PathSegment, Token, Type, Visibility,
};

use quote::{quote, ToTokens};

#[proc_macro_derive(Builder, attributes(builder))]
pub fn derive(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    let base_ident = &input.ident;
    let builder_ident = Ident::new(&format!("{}Builder", base_ident), Span::call_site());

    let fields = extract_fields(&input);
    let defaults = builder_defaults(&fields);
    let setters = builder_setters(&fields);
    let builder_fields = builder_fields(&fields);
    let build_func = builder_build(&fields, &base_ident, &builder_ident);

    let ast = quote! {
        use std::error::Error;

        pub struct #builder_ident {
            #builder_fields
        }

        impl #base_ident {
            pub fn builder() -> #builder_ident {
                #builder_ident { #defaults }
            }
        }

        impl #builder_ident {
            #setters

            #build_func
        }
    };

    ast.into()
}

fn extract_fields(input: &DeriveInput) -> &Fields {
    match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(..) => &data.fields,
            _ => unreachable!(),
        },
        _ => unreachable!(),
    }
}

fn builder_fields(fields: &Fields) -> TokenStream {
    let fields: Vec<_> = fields
        .iter()
        .map(|f| {
            let vis = &f.vis;
            let ty = &f.ty;
            let ident = &f.ident;
            let colon_token = &f.colon_token;

            let new_ty = if is_optional(f) || is_builder_field(f) {
                quote! { #ty }
            } else {
                quote! { std::option::Option< #ty >}
            };

            quote! {
                #vis #ident #colon_token #new_ty
            }
        })
        .collect();

    let ast = quote! {
        #(#fields),*
    };

    ast.into()
}

fn builder_setters(fields: &Fields) -> TokenStream {
    let setters: Vec<_> = fields
        .iter()
        .map(|f| {
            let name = &f.ident;

            if is_optional(f) {
                let ty = optional_inner_ty(f);

                quote! {
                    fn #name(&mut self, #name: #ty) -> &mut Self {
                      self.#name = std::option::Option::Some(#name);
                      self
                    }
                }
            } else {
                let ty = &f.ty;

                if is_builder_field(f) {
                    let each = builder_attr_each(f);

                    quote! {
                        fn #each(&mut self, #each: String) -> &mut Self {
                          self.#name.push(#each);
                          self
                        }
                    }
                } else {
                    quote! {
                        fn #name(&mut self, #name: #ty) -> &mut Self {
                          self.#name = std::option::Option::Some(#name);
                          self
                        }
                    }
                }
            }
        })
        .collect();

    let ast = quote! {
        #(#setters)*
    };

    ast.into()
}

fn builder_defaults(fields: &Fields) -> TokenStream {
    let defaults: Vec<_> = fields
        .iter()
        .map(|f| {
            let ident = &f.ident;
            let colon_token = &f.colon_token;

            if is_builder_field(f) {
                quote! {
                    #ident #colon_token Vec::new()
                }
            } else {
                quote! {
                    #ident #colon_token std::option::Option::None
                }
            }
        })
        .collect();

    let ast = quote! {
        #(#defaults),*
    };

    ast.into()
}

fn builder_build(fields: &Fields, base_ident: &Ident, builder_ident: &Ident) -> TokenStream {
    let missing_values_checkers: Vec<TokenStream> = fields
        .iter()
        .filter(|f| is_optional(f) == false && is_builder_field(f) == false)
        .map(|f| {
            let ident = f.ident.as_ref().unwrap();
            let ident_str = format!("{}", ident);

            quote! {
                if self.#ident.is_none() {
                    let msg = format!("Field `{}` has a no value", #ident_str);
                    return Err(msg.into());
                }
            }
        })
        .collect();

    let locals: Vec<TokenStream> = fields
        .iter()
        .map(|f| {
            let ident: &Ident = &f.ident.as_ref().unwrap();
            let local = Ident::new(&format!("{}_local", &ident), Span::call_site());

            if is_builder_field(f) {
                quote! {
                    let #local = self.#ident.clone();
                }
            } else if is_optional(f) {
                quote! {
                    let #local =
                        if self.#ident.is_some() {
                            self.#ident.take()
                        }
                        else {
                            std::option::Option::None
                        };
                }
            } else {
                quote! {
                    let #local = self.#ident.take().unwrap();
                }
            }
        })
        .collect();

    let initializers: Vec<TokenStream> = fields
        .iter()
        .map(|f| {
            let ident: &Ident = &f.ident.as_ref().unwrap();

            let local = Ident::new(&format!("{}_local", &ident), Span::call_site());

            quote! {
                #ident: #local
            }
        })
        .collect();

    quote! {
        pub fn build(&mut self) -> std::result::Result<#base_ident, std::boxed::Box<dyn std::error::Error>> {
            self.assert_non_missing_values()?;

            #(#locals)*

            let mut instance = #base_ident {
                #(#initializers),*
            };

            Ok(instance)
        }

        fn assert_non_missing_values(&self) -> std::result::Result<(), std::boxed::Box<dyn std::error::Error>> {
            #(#missing_values_checkers)*

            Ok(())
        }
    }
}

fn is_optional(f: &Field) -> bool {
    if let Type::Path(ref path) = &f.ty {
        let path: &Path = &path.path;
        let segment: &PathSegment = &path.segments.first().unwrap();
        let ident = &segment.ident;

        if &format!("{}", ident) == "Option" {
            return true;
        }
    }
    false
}

fn is_builder_field(f: &Field) -> bool {
    f.attrs.iter().any(is_builder_attr)
}

fn is_builder_attr(attr: &Attribute) -> bool {
    let meta: Meta = attr.parse_meta().unwrap();

    match meta {
        Meta::List(list) => {
            let path = &list.path;
            let segment: &PathSegment = &path.segments.first().unwrap();

            let ident = format!("{}", &segment.ident);

            if ident == "builder" {
                return true;
            }
        }
        _ => (),
    }

    false
}

fn optional_inner_ty(f: &Field) -> TokenStream {
    if let Type::Path(ref path) = &f.ty {
        let path: &Path = &path.path;
        let segments = &path.segments;

        let n = segments.len();
        assert_eq!(n, 1);

        let args = &segments[0].arguments;

        match args {
            PathArguments::AngleBracketed(args) => {
                let args = &args.args;

                return quote! {
                    #args
                };
            }
            _ => unreachable!(),
        };
    }

    unreachable!()
}

fn builder_attr_each(f: &Field) -> Ident {
    for attr in f.attrs.iter() {
        let meta: Meta = attr.parse_meta().unwrap();

        match meta {
            Meta::List(list) => {
                let path = &list.path;
                let segment: &PathSegment = &path.segments.first().unwrap();

                let ident = format!("{}", &segment.ident);

                if ident == "builder" {
                    let nested: &Punctuated<NestedMeta, Token![,]> = &list.nested;
                    assert_eq!(nested.len(), 1);

                    let nested: &NestedMeta = nested.first().unwrap();

                    match nested {
                        NestedMeta::Meta(Meta::NameValue(nv)) => match nv.lit {
                            Lit::Str(ref lit_str) => {
                                let lit_str = &lit_str.value();

                                if lit_str != "each" {
                                    // compile_error!(r#"expected `builder(each = "...")`"#);
                                }

                                return Ident::new(&lit_str, Span::call_site());
                            }
                            _ => unreachable!(),
                        },
                        _ => unreachable!(),
                    }
                }
            }
            _ => continue,
        }
    }

    unreachable!()
}
