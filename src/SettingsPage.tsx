// src/SettingsPage.tsx
import React, {useEffect, useState} from 'react';
import {
    Box,
    Button,
    CircularProgress,
    Container,
    FormControl,
    InputLabel,
    MenuItem,
    Paper,
    Select,
    SelectChangeEvent,
    Typography
} from '@mui/material';
import i18n from "i18next";
import {useTranslation} from 'react-i18next';
import {invoke} from "@tauri-apps/api/core";
import {invokeTauriCommandWrapper, isGpuProfile} from "./utils.ts";
import {ThemeModeSetting} from "./App.tsx";

interface StatusUpdateProps {
    updateStatus: (newStatus: { error?: string | null, info?: string | null, messageLoading?: boolean }) => void;
    clearMessages: () => void;
}

interface SettingsPageProps extends StatusUpdateProps {
    currentTheme: ThemeModeSetting;
    onChangeTheme: (theme: ThemeModeSetting) => void;
    onBack: () => void;
    currentProfile?: string | null;
}

interface ConfigItemFromRust {
    name: string;
    description: string;
    value: string | number | boolean;
    default_value: string | number | boolean;
    options?: (string | number | boolean)[];
}
const PIP_CACHE_DIR_CONFIG_KEY = "Pip Cache Directory";
const PIP_INDEX_URL_CONFIG_KEY = "Pip Index URL";
const PIP_TORCH_INDEX_URL_CONFIG_KEY = "Pip Torch Index URL";
const PYTHON_SOURCE_CONFIG_KEY = "Python Source";
const LANGUAGE_CONFIG_KEY = "Language";

const languageNames: { [key: string]: string } = { 'en': 'English', 'zh-CN': '简体中文', 'zh-TW': '繁體中文', 'es': 'Español', 'ja': '日本語', 'ko': '한국어' };

const getPipIndexUrlName = (url: string, t: (key: string) => string) => {
    if (url === '') return t('System Default');
    if (url.includes('pypi.org')) return t('PyPI');
    if (url.includes('tsinghua')) return t('Tsinghua');
    if (url.includes('aliyun')) return t('Aliyun');
    if (url.includes('ustc')) return t('USTC');
    if (url.includes('huaweicloud')) return t('Huawei Cloud');
    if (url.includes('tencent')) return t('Tencent Cloud');
    return url;
};

const getTorchIndexUrlName = (url: string, t: (key: string) => string) => {
    if (url === '') return t('System Default');
    if (url.includes('pytorch.org')) return t('Official PyTorch');
    if (url.includes('nju.edu.cn')) return t('NJU Mirror');
    return url;
};

const getPythonSourceUrlName = (url: string, t: (key: string) => string) => {
    if (url === '') return t('uv default');
    if (url.includes('github.com')) return t('GitHub');
    return url;
};

const SettingsPage: React.FC<SettingsPageProps> = ({ currentTheme, onChangeTheme, onBack, updateStatus, clearMessages, currentProfile }) => {
    const {t} = useTranslation();
    const [configs, setConfigs] = useState<ConfigItemFromRust[] | null>(null);
    const [isLoading, setIsLoading] = useState(true);

    const loadConfigs = async () => {
        setIsLoading(true);
        await invokeTauriCommandWrapper<ConfigItemFromRust[]>(
            'get_config_payload', undefined,
            (result) => {
                setConfigs(result);
                const languageConfig = result.find(c => c.name === LANGUAGE_CONFIG_KEY);
                if (languageConfig && languageConfig.value) {
                    i18n.changeLanguage(languageConfig.value as string);
                }
            },
            (errorMsg) => updateStatus({error: `Failed to load settings: ${errorMsg}`})
        );
        setIsLoading(false);
    };

    useEffect(() => {
        loadConfigs();
    }, []);

    const handleSettingChange = async (name: string, value: string | number) => {
        clearMessages();
        updateStatus({messageLoading: true});
        await invokeTauriCommandWrapper<void>(
            'update_config_item', {name, value},
            async () => {
                const updatedConfigs = await invoke<ConfigItemFromRust[]>('get_config_payload');
                setConfigs(updatedConfigs);
                if (name === LANGUAGE_CONFIG_KEY) i18n.changeLanguage(value as string);
                updateStatus({info: `${name} updated successfully.`, messageLoading: false});
            },
            (errorMsg) => updateStatus({error: `Failed to update ${name}: ${errorMsg}`, messageLoading: false})
        );
    };

    if (isLoading || !configs) {
        return (
            <Container maxWidth="sm" sx={{py: 4, display: 'flex', justifyContent: 'center', alignItems: 'center', height: '100vh'}}>
                <CircularProgress/><Typography sx={{ml: 2}}>{t('Loading settings...')}</Typography>
            </Container>
        );
    }

    const getConfig = (key: string) => configs.find(c => c.name === key);
    const languageConfig = getConfig(LANGUAGE_CONFIG_KEY);
    const themeConfig = { value: currentTheme, options: ['system', 'light', 'dark'] };
    const pipCacheConfig = getConfig(PIP_CACHE_DIR_CONFIG_KEY);
    const pipIndexUrlConfig = getConfig(PIP_INDEX_URL_CONFIG_KEY);
    const pipTorchIndexConfig = getConfig(PIP_TORCH_INDEX_URL_CONFIG_KEY);
    const pythonSourceConfig = getConfig(PYTHON_SOURCE_CONFIG_KEY);
    // The PyTorch CUDA (cu126) source is only meaningful for the GPU variant;
    // show it only when the installed profile is GPU.
    const showTorchPicker = !!pipTorchIndexConfig && isGpuProfile(currentProfile);

    return (
        <Container maxWidth="sm" sx={{py: 4}}>
            <Paper elevation={3} sx={{p: 3}}>
                <Typography variant="h4" component="h1" gutterBottom align="center">{t('Settings')}</Typography>
                {[
                    { label: t('Language'), config: languageConfig, handler: (e: SelectChangeEvent) => handleSettingChange(LANGUAGE_CONFIG_KEY, e.target.value), renderOption: (o: string) => languageNames[o] || o },
                    { label: t('Theme'), config: themeConfig, handler: (e: SelectChangeEvent) => onChangeTheme(e.target.value as ThemeModeSetting), renderOption: (o: string) => t(o.charAt(0).toUpperCase() + o.slice(1)) },
                    { label: t('Pip Cache Directory'), config: pipCacheConfig, handler: (e: SelectChangeEvent) => handleSettingChange(PIP_CACHE_DIR_CONFIG_KEY, e.target.value), renderOption: (o: string) => t(o) },
                    { label: t('Pip Index URL'), config: pipIndexUrlConfig, handler: (e: SelectChangeEvent) => handleSettingChange(PIP_INDEX_URL_CONFIG_KEY, e.target.value), renderOption: (o: string) => getPipIndexUrlName(o, t) },
                    { label: t('Python Source'), config: pythonSourceConfig, handler: (e: SelectChangeEvent) => handleSettingChange(PYTHON_SOURCE_CONFIG_KEY, e.target.value), renderOption: (o: string) => getPythonSourceUrlName(o, t), helperText: t('Used only when a new Python runtime is installed.') },
                    ...(showTorchPicker ? [{ label: t('Torch Source'), config: pipTorchIndexConfig, handler: (e: SelectChangeEvent) => handleSettingChange(PIP_TORCH_INDEX_URL_CONFIG_KEY, e.target.value), renderOption: (o: string) => getTorchIndexUrlName(o, t), helperText: t('Only applies to the next install or profile change.') }] : []),
                ].map(({ label, config, handler, renderOption, helperText }) => config && (
                    <Box key={label} sx={{my: 2}}>
                        <FormControl fullWidth variant="outlined">
                            <InputLabel>{label}</InputLabel>
                            <Select value={(config.value as string) || ''} label={label} onChange={handler}>
                                {(config.options as string[])?.map(o => <MenuItem key={o} value={o}>{renderOption(o)}</MenuItem>)}
                            </Select>
                        </FormControl>
                        {helperText && (<Typography variant="caption" color="text.secondary" sx={{display: 'block', mt: 0.5}}>{helperText}</Typography>)}
                    </Box>
                ))}
                <Box sx={{mt: 4, display: 'flex', justifyContent: 'center'}}>
                    <Button variant="outlined" onClick={onBack}>{t('Back to App')}</Button>
                </Box>
            </Paper>
        </Container>
    );
};
export default SettingsPage;
