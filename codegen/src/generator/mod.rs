use inflector::Inflector;

use botocore::{Service, Shape, Operation};
use std::ascii::AsciiExt;
use self::ec2::Ec2Generator;
use self::json::JsonGenerator;
use self::query::QueryGenerator;
use self::rest_json::RestJsonGenerator;

mod ec2;
mod json;
mod query;
mod rest_json;

pub trait GenerateProtocol {
    fn generate_methods(&self, service: &Service) -> String;

    fn generate_prelude(&self, service: &Service) -> String;

    fn generate_struct_attributes(&self) -> String;

    fn generate_support_types(&self, _name: &str, _shape: &Shape, _service: &Service)
        -> Option<String> {
        None
    }

    fn generate_error_types(&self, _service: &Service) -> Option<String> {
        None
    }

    fn timestamp_type(&self) -> &'static str;
}

pub fn generate_source(service: &Service) -> String {
    match &service.metadata.protocol[..] {
        "json" => generate(service, JsonGenerator),
        "ec2" => generate(service, Ec2Generator),
        "query" => generate(service, QueryGenerator),
        "rest-json" => generate(service, RestJsonGenerator),
        protocol => panic!("Unknown protocol {}", protocol),
    }
}

fn generate<P>(service: &Service, protocol_generator: P) -> String where P: GenerateProtocol {
    format!(
        "{prelude}

        {types}
        {error_types}

        {client}",
        client = generate_client(service, &protocol_generator),
        prelude = &protocol_generator.generate_prelude(service),
        types = generate_types(service, &protocol_generator),
        error_types = protocol_generator.generate_error_types(service).unwrap_or("".to_string()),
    )
}

fn generate_client<P>(service: &Service, protocol_generator: &P) -> String
where P: GenerateProtocol {
    format!(
        "/// A client for the {service_name} API.
        pub struct {type_name}<P> where P: ProvideAwsCredentials {{
            credentials_provider: P,
            region: region::Region,
        }}

        impl<P> {type_name}<P> where P: ProvideAwsCredentials {{
            pub fn new(credentials_provider: P, region: region::Region) -> Self {{
                {type_name} {{
                    credentials_provider: credentials_provider,
                    region: region,
                }}
            }}

            {methods}
        }}
        ",
        methods = protocol_generator.generate_methods(service),
        service_name = match &service.metadata.service_abbreviation {
            &Some(ref service_abbreviation) => service_abbreviation.as_str(),
            &None => service.metadata.service_full_name.as_ref()
        },
        type_name = service.client_type_name(),
    )
}

fn generate_list(name: &str, shape: &Shape) -> String {
    format!("pub type {} = Vec<{}>;", name, capitalize_first(shape.member().to_string()))
}

fn generate_map(name: &str, shape: &Shape) -> String {
    format!(
        "pub type {} = ::std::collections::HashMap<{}, {}>;",
        name,
        capitalize_first(shape.key().to_string()),
        capitalize_first(shape.value().to_string()),
    )
}

fn generate_primitive_type(name: &str, shape_type: &str, for_timestamps: &str) -> String {
    let primitive_type = match shape_type {
        "blob" => "Vec<u8>",
        "boolean" => "bool",
        "double" => "f64",
        "float" => "f32",
        "integer" => "i32",
        "long" => "i64",
        "string" => "String",
        "timestamp" => for_timestamps,
        primitive_type => panic!("Unknown primitive type: {}", primitive_type),
    };

    format!("pub type {} = {};", name, primitive_type)
}

fn generate_types<P>(service: &Service, protocol_generator: &P) -> String
where P: GenerateProtocol {
    service.shapes.iter().filter_map(|(name, shape)| {
        let type_name = &capitalize_first(name.to_string());

        if type_name == "String" {
            return protocol_generator.generate_support_types(type_name, shape, &service);
        }

        if shape.exception() && service.typed_errors() {
            return None;
        }



        let mut parts = Vec::with_capacity(3);

        if let Some(ref docs) = shape.documentation {
            parts.push(format!("#[doc=\"{}\"]", docs.replace("\\","\\\\").replace("\"", "\\\"")));
        }

        match &shape.shape_type[..] {
            "structure" => parts.push(generate_struct(service, type_name, shape, protocol_generator)),
            "map" => parts.push(generate_map(type_name, shape)),
            "list" => parts.push(generate_list(type_name, shape)),
            shape_type => parts.push(generate_primitive_type(type_name, shape_type, protocol_generator.timestamp_type())),
        }

        if let Some(support_types) = protocol_generator.generate_support_types(type_name, shape, &service) {
            parts.push(support_types);
        }

        Some(parts.join("\n"))
    }).collect::<Vec<String>>().join("\n")
}



fn generate_struct<P>(
    service: &Service,
    name: &str,
    shape: &Shape,
    protocol_generator: &P,
) -> String where P: GenerateProtocol {
    if shape.members.is_none() || shape.members.as_ref().unwrap().is_empty() {
        format!(
            "{attributes}
            pub struct {name};
            ",
            attributes = protocol_generator.generate_struct_attributes(),
            name = name,
        )
    } else {
        format!(
            "{attributes}
            pub struct {name} {{
                {struct_fields}
            }}
            ",
            attributes = protocol_generator.generate_struct_attributes(),
            name = name,
            struct_fields = generate_struct_fields(service, shape),
        )
    }

}

pub fn generate_field_name(member_name: &str) -> String {
    let name = member_name.to_snake_case();
    if name == "return" || name == "type" {
        name + "_"
    } else {
        name
    }
}

fn generate_struct_fields(service: &Service, shape: &Shape) -> String {
    shape.members.as_ref().unwrap().iter().map(|(member_name, member)| {
        let mut lines = Vec::with_capacity(4);
        let name = generate_field_name(member_name);

        if let Some(ref docs) = member.documentation {
            lines.push(format!("#[doc=\"{}\"]", docs.replace("\\","\\\\").replace("\"", "\\\"")));
        }

        lines.push("#[allow(unused_attributes)]".to_owned());
        lines.push(format!("#[serde(rename=\"{}\")]", member_name));

        if let Some(shape_type) = service.shape_type_for_member(member) {
            if shape_type == "blob" {
                lines.push(
                    "#[serde(
                        deserialize_with=\"::serialization::SerdeBlob::deserialize_blob\",
                        serialize_with=\"::serialization::SerdeBlob::serialize_blob\",
                        default,
                    )]".to_owned()
                );
            } else if shape_type == "boolean" && !shape.required(member_name) {
                lines.push("#[serde(skip_serializing_if=\"::std::option::Option::is_none\")]".to_owned());
            }
        }

        let type_name = capitalize_first(member.shape.to_string());

        if shape.required(member_name) {
            lines.push(format!("pub {}: {},",  name, type_name));
        } else if name == "type" {
            lines.push(format!("pub aws_{}: Option<{}>,",  name, type_name));
        } else {
            lines.push(format!("pub {}: Option<{}>,",  name, type_name));
        }

        lines.join("\n")
    }).collect::<Vec<String>>().join("\n")
}

impl Operation {
    pub fn error_type_name(&self) -> String {
        format!("{}Error", self.name)
    }
}

fn capitalize_first(word: String) -> String {
    assert!(word.is_ascii());

    let mut result = word.into_bytes();
    result[0] = result[0].to_ascii_uppercase();

    String::from_utf8(result).unwrap()
}